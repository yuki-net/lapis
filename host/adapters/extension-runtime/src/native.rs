//! 検証済み native 拡張を拡張ホスト内で実行する ABI アダプター。
//!
//! package の検証と配置は呼び出し側が担当する。未検証のファイルには使用しない。

use std::{
    ffi::c_void,
    path::Path,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

use extension_native_api::{
    ABI_VERSION, ByteView, ENTRY_SYMBOL, EntryV1, ExtensionApiV1, HostApiV1, MAX_MESSAGE_BYTES,
    OwnedBytes, STATUS_ERROR, STATUS_OK,
};
use serde_json::Value;

use crate::InvocationContext;

/// 認証された拡張に対する Host 能力の仲介。許可判定は呼び出し側が行う。
pub type NativeBroker =
    Arc<dyn Fn(&str, Value, Option<&InvocationContext>) -> Result<Value, String> + Send + Sync>;

struct HostState {
    broker: NativeBroker,
    thread: ThreadId,
    invocation: Mutex<Option<InvocationContext>>,
}

struct InvocationGuard<'a>(&'a HostState);

impl Drop for InvocationGuard<'_> {
    fn drop(&mut self) {
        *self
            .0
            .invocation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }
}

/// ライブラリとインスタンスは同じ thread で保持し、操作を直列化する。
pub struct NativeExtension {
    library: libloading::Library,
    api: *const ExtensionApiV1,
    instance: *mut c_void,
    _host_state: Box<HostState>,
    _host_api: Box<HostApiV1>,
    #[cfg(windows)]
    _image: Option<StagedImage>,
}

#[cfg(windows)]
struct StagedImage {
    _file: std::fs::File,
    _directory_lock: std::fs::File,
    _directory: tempfile::TempDir,
}

impl NativeExtension {
    /// 署名と版を確認し、検証後に変更されない絶対パスだけを読み込む。
    ///
    /// # Safety
    /// 読み込むファイルは実行コードである。呼び出し側は署名とファイルの同一性を
    /// 検証し、依存ライブラリを含めた読み込み先を信頼しなければならない。
    pub unsafe fn load_verified(path: &Path, broker: NativeBroker) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("native extension path must be absolute".into());
        }
        let library =
            unsafe { libloading::Library::new(path) }.map_err(|error| error.to_string())?;
        unsafe { Self::activate_library(library, broker) }
    }

    /// 検証済み bytes を書き換え禁止の一時 file に固定してから読み込む。
    ///
    /// # Safety
    /// bytes が信頼できる実行コードであることを呼び出し側が保証する。
    #[cfg(windows)]
    pub unsafe fn load_verified_bytes(bytes: &[u8], broker: NativeBroker) -> Result<Self, String> {
        use std::{
            io::{Read, Write},
            os::windows::fs::OpenOptionsExt,
        };
        if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
            return Err("invalid native extension image size".into());
        }
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let directory_lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x02000000)
            .open(directory.path())
            .map_err(|error| error.to_string())?;
        let path = directory.path().join("extension.dll");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(1)
            .open(&path)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        drop(file);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .map_err(|error| error.to_string())?;
        let mut actual = Vec::new();
        Read::by_ref(&mut file)
            .take((32 * 1024 * 1024 + 1) as u64)
            .read_to_end(&mut actual)
            .map_err(|error| error.to_string())?;
        if actual != bytes {
            return Err("staged native extension does not match verified image".into());
        }
        // 依存 DLL の探索先は system32 に限定する。
        let library =
            unsafe { libloading::os::windows::Library::load_with_flags(&path, 0x00000800) }
                .map_err(|error| error.to_string())?;
        let mut extension = unsafe { Self::activate_library(library.into(), broker) }?;
        extension._image = Some(StagedImage {
            _file: file,
            _directory_lock: directory_lock,
            _directory: directory,
        });
        Ok(extension)
    }

    unsafe fn activate_library(
        library: libloading::Library,
        broker: NativeBroker,
    ) -> Result<Self, String> {
        let entry: libloading::Symbol<EntryV1> = unsafe { library.get(ENTRY_SYMBOL) }
            .map_err(|_| "native extension entry not found".to_owned())?;
        let api = unsafe { entry() };
        if api.is_null() {
            return Err("invalid native extension API".into());
        }
        let version = unsafe { std::ptr::addr_of!((*api).abi_version).read() };
        let size = unsafe { std::ptr::addr_of!((*api).struct_size).read() };
        if version != ABI_VERSION || (size as usize) < std::mem::size_of::<ExtensionApiV1>() {
            return Err("incompatible native extension ABI".into());
        }
        let table = unsafe { &*api };
        if table.activate.is_none()
            || table.dispatch.is_none()
            || table.deactivate.is_none()
            || table.release.is_none()
        {
            return Err("incomplete native extension ABI".into());
        }
        let mut host_state = Box::new(HostState {
            broker,
            thread: thread::current().id(),
            invocation: Mutex::new(None),
        });
        let host_api = Box::new(HostApiV1 {
            abi_version: ABI_VERSION,
            struct_size: std::mem::size_of::<HostApiV1>() as u32,
            context: (&mut *host_state as *mut HostState).cast(),
            call: Some(host_call),
            release: Some(host_release),
        });
        let mut instance = std::ptr::null_mut();
        let mut output = OwnedBytes::EMPTY;
        let status = unsafe { table.activate.unwrap()(&*host_api, &mut instance, &mut output) };
        let message = unsafe { take_output(table, output) };
        if status != STATUS_OK || instance.is_null() || message.is_err() {
            if !instance.is_null() {
                let mut cleanup = OwnedBytes::EMPTY;
                unsafe { table.deactivate.unwrap()(instance, &mut cleanup) };
                let _ = unsafe { take_output(table, cleanup) };
            }
            let message = message?;
            return Err(if message.is_empty() {
                "native extension activation failed".into()
            } else {
                message
            });
        }
        Ok(Self {
            library,
            api,
            instance,
            _host_state: host_state,
            _host_api: host_api,
            #[cfg(windows)]
            _image: None,
        })
    }

    pub fn dispatch(
        &mut self,
        operation: &str,
        payload: &str,
        context: &InvocationContext,
    ) -> Result<String, String> {
        if self.instance.is_null() {
            return Err("native extension is deactivated".into());
        }
        if !context.valid()
            || operation.is_empty()
            || operation.len() > 256
            || payload.len() > MAX_MESSAGE_BYTES
        {
            return Err("invalid dispatch request".into());
        }
        *self
            ._host_state
            .invocation
            .lock()
            .map_err(|_| "native invocation state unavailable")? = Some(context.clone());
        let _invocation = InvocationGuard(&self._host_state);
        let table = unsafe { &*self.api };
        let mut output = OwnedBytes::EMPTY;
        let status = unsafe {
            table.dispatch.unwrap()(
                self.instance,
                ByteView::from_bytes(operation.as_bytes()),
                ByteView::from_bytes(payload.as_bytes()),
                ByteView::from_bytes(context.session.as_bytes()),
                ByteView::from_bytes(context.workspace.as_bytes()),
                &mut output,
            )
        };
        let output = unsafe { take_output(table, output) }?;
        if status == STATUS_OK {
            Ok(output)
        } else {
            Err(output)
        }
    }

    pub fn deactivate(&mut self) -> Result<(), String> {
        if self.instance.is_null() {
            return Ok(());
        }
        let instance = std::mem::replace(&mut self.instance, std::ptr::null_mut());
        let table = unsafe { &*self.api };
        let mut output = OwnedBytes::EMPTY;
        let status = unsafe { table.deactivate.unwrap()(instance, &mut output) };
        let output = unsafe { take_output(table, output) }?;
        if status == STATUS_OK {
            Ok(())
        } else {
            Err(output)
        }
    }
}

impl Drop for NativeExtension {
    fn drop(&mut self) {
        let _ = self.deactivate();
        // 関数表とインスタンスの解放より先にライブラリを unload しない。
        let _ = &self.library;
    }
}

unsafe fn take_output(table: &ExtensionApiV1, output: OwnedBytes) -> Result<String, String> {
    let result = if output.len > MAX_MESSAGE_BYTES || (output.len == 0) != output.ptr.is_null() {
        Err("invalid native extension output".into())
    } else {
        let bytes = if output.len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(output.ptr, output.len) }
        };
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| "invalid native extension UTF-8".into())
    };
    unsafe { table.release.unwrap()(output) };
    result
}

unsafe fn view(value: ByteView) -> Result<Vec<u8>, String> {
    if value.len > MAX_MESSAGE_BYTES || (value.len == 0) != value.ptr.is_null() {
        return Err("invalid native Host request".into());
    }
    Ok(if value.len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(value.ptr, value.len) }.to_vec()
    })
}

unsafe extern "C" fn host_call(
    context: *mut c_void,
    method: ByteView,
    payload: ByteView,
    output: *mut OwnedBytes,
) -> i32 {
    if context.is_null() || output.is_null() {
        return STATUS_ERROR;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = unsafe { &*context.cast::<HostState>() };
        if state.thread != thread::current().id() {
            return Err("native Host calls must use the calling thread".to_owned());
        }
        let method =
            String::from_utf8(unsafe { view(method)? }).map_err(|_| "invalid method UTF-8")?;
        if method.is_empty() || method.len() > 256 {
            return Err("invalid method".to_owned());
        }
        let payload = serde_json::from_slice(&unsafe { view(payload)? })
            .map_err(|_| "invalid Host request JSON")?;
        let invocation = state
            .invocation
            .lock()
            .map_err(|_| "native invocation state unavailable")?
            .clone();
        let response = (state.broker)(&method, payload, invocation.as_ref())?;
        serde_json::to_vec(&response).map_err(|error| error.to_string())
    }));
    let (status, bytes) = match result {
        Ok(Ok(bytes)) if bytes.len() <= MAX_MESSAGE_BYTES => (STATUS_OK, bytes),
        Ok(Ok(_)) => (STATUS_ERROR, b"Host response too large".to_vec()),
        Ok(Err(error)) => (STATUS_ERROR, error.into_bytes()),
        Err(_) => (STATUS_ERROR, b"Host capability failed".to_vec()),
    };
    let bytes = if bytes.len() > MAX_MESSAGE_BYTES {
        b"Host error too large".to_vec()
    } else {
        bytes
    };
    let mut bytes = bytes.into_boxed_slice();
    let result = if bytes.is_empty() {
        OwnedBytes::EMPTY
    } else {
        let result = OwnedBytes {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
        };
        std::mem::forget(bytes);
        result
    };
    unsafe {
        *output = result;
    }
    status
}

unsafe extern "C" fn host_release(_: *mut c_void, value: OwnedBytes) {
    if !value.ptr.is_null() && value.len != 0 {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(value.ptr, value.len)) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[cfg(windows)]
    #[test]
    #[ignore = "cargo build --locked --manifest-path examples/native-extension/Cargo.toml --target-dir target/native-extension が必要"]
    fn staged_image_is_locked_until_extension_is_unloaded() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/native-extension/debug/lapis_native_example.dll");
        let bytes = std::fs::read(path).unwrap();
        let broker = Arc::new(|_: &str, _: Value, _: Option<&InvocationContext>| Ok(Value::Null));
        let extension = unsafe { NativeExtension::load_verified_bytes(&bytes, broker) }.unwrap();
        let directory = extension
            ._image
            .as_ref()
            .unwrap()
            ._directory
            .path()
            .to_owned();
        let image = directory.join("extension.dll");
        assert!(std::fs::write(&image, b"modified").is_err());
        assert!(std::fs::remove_file(&image).is_err());
        assert!(std::fs::rename(&directory, directory.with_extension("moved")).is_err());
        drop(extension);
        assert!(!directory.exists());
    }

    #[test]
    fn other_threads_cannot_borrow_an_active_invocation() {
        let called = Arc::new(AtomicBool::new(false));
        let observed = called.clone();
        let state = HostState {
            broker: Arc::new(move |_, _, _| {
                observed.store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }),
            thread: thread::current().id(),
            invocation: Mutex::new(Some(InvocationContext {
                session: "session".into(),
                workspace: "workspace".into(),
            })),
        };
        let pointer = (&state as *const HostState) as usize;
        thread::scope(|scope| {
            scope.spawn(move || {
                let mut output = OwnedBytes::EMPTY;
                let status = unsafe {
                    host_call(
                        pointer as *mut c_void,
                        ByteView::from_bytes(b"pty.v1.spawn"),
                        ByteView::from_bytes(b"{}"),
                        &mut output,
                    )
                };
                assert_eq!(status, STATUS_ERROR);
                unsafe { host_release(std::ptr::null_mut(), output) };
            });
        });
        assert!(!called.load(Ordering::SeqCst));
    }
}

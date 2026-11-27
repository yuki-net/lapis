//! 汎用 native 拡張 ABI v1 の最小例。
//!
//! Terminal には依存せず、`echo.v1.echo` だけを公開する。native 拡張基盤の
//! 動作確認用で、配布はしない。

use extension_native_api::{
    ABI_VERSION, ByteView, ExtensionApiV1, HostApiV1, MAX_MESSAGE_BYTES, OwnedBytes, STATUS_ERROR,
    STATUS_OK,
};
use serde_json::{Value, json};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};

struct NativeHost(HostApiV1);

impl NativeHost {
    fn call(&self, method: &str, payload: Value) -> Result<Value, String> {
        let bytes = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("host request too large".into());
        }
        let mut output = OwnedBytes::EMPTY;
        // Host は有効な関数表と context を提供し、返却領域を release まで保持する。
        let status = unsafe {
            self.0.call.ok_or("host call unavailable")?(
                self.0.context,
                ByteView::from_bytes(method.as_bytes()),
                ByteView::from_bytes(&bytes),
                &mut output,
            )
        };
        let result = unsafe { copy_output(output) };
        unsafe { self.0.release.ok_or("host release unavailable")?(self.0.context, output) };
        let result = result?;
        if status != STATUS_OK {
            return Err(String::from_utf8_lossy(&result).into_owned());
        }
        serde_json::from_slice(&result).map_err(|_| "invalid Host result".into())
    }
}

struct Echo {
    host: NativeHost,
}

impl Echo {
    fn dispatch(&self, operation: &str, payload: &str) -> Result<String, String> {
        match operation {
            "echo.v1.echo" => {
                let request: Value =
                    serde_json::from_str(payload).map_err(|_| "invalid payload")?;
                let value = request.get("value").cloned().ok_or("missing value")?;
                // Host の broker を経由して同じ payload を往復させる。
                let response = self.host.call("echo.v1.echo", json!({"value": value}))?;
                serde_json::to_string(&response).map_err(|error| error.to_string())
            }
            _ => Err(format!("unknown operation: {operation}")),
        }
    }
}

// ABI の呼び出し元は有効な領域を渡す。長さと null の組み合わせは境界で検証する。
unsafe fn copy_output(value: OwnedBytes) -> Result<Vec<u8>, String> {
    if value.len > MAX_MESSAGE_BYTES || (value.len == 0) != value.ptr.is_null() {
        return Err("invalid ABI output".into());
    }
    if value.len == 0 {
        return Ok(Vec::new());
    }
    Ok(unsafe { std::slice::from_raw_parts(value.ptr, value.len) }.to_vec())
}

unsafe fn read_view(value: &ByteView) -> Result<&str, String> {
    if value.len > MAX_MESSAGE_BYTES || (value.len == 0) != value.ptr.is_null() {
        return Err("invalid ABI input".into());
    }
    let bytes = if value.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(value.ptr, value.len) }
    };
    std::str::from_utf8(bytes).map_err(|_| "invalid UTF-8".into())
}

fn return_bytes(value: Vec<u8>, out: *mut OwnedBytes) {
    let mut value = value.into_boxed_slice();
    let output = if value.is_empty() {
        OwnedBytes::EMPTY
    } else {
        let output = OwnedBytes {
            ptr: value.as_mut_ptr(),
            len: value.len(),
        };
        std::mem::forget(value);
        output
    };
    unsafe {
        *out = output;
    }
}

fn boundary(out: *mut OwnedBytes, operation: impl FnOnce() -> Result<String, String>) -> i32 {
    if out.is_null() {
        return STATUS_ERROR;
    }
    let (mut status, result) = match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => (STATUS_OK, value),
        Ok(Err(error)) => (STATUS_ERROR, error),
        Err(_) => (STATUS_ERROR, "extension panicked".into()),
    };
    let result = if result.len() > MAX_MESSAGE_BYTES {
        status = STATUS_ERROR;
        "extension response too large".to_owned()
    } else {
        result
    };
    return_bytes(result.into_bytes(), out);
    status
}

unsafe extern "C" fn activate(
    host: *const HostApiV1,
    instance: *mut *mut c_void,
    out: *mut OwnedBytes,
) -> i32 {
    boundary(out, || {
        if host.is_null() || instance.is_null() {
            return Err("invalid ABI arguments".into());
        }
        unsafe {
            *instance = std::ptr::null_mut();
        }
        // 先頭の版・サイズだけを読み、全体が v1 以上であることを確かめる。
        let version = unsafe { std::ptr::addr_of!((*host).abi_version).read() };
        let size = unsafe { std::ptr::addr_of!((*host).struct_size).read() };
        if version != ABI_VERSION || (size as usize) < std::mem::size_of::<HostApiV1>() {
            return Err("incompatible Host ABI".into());
        }
        let host = unsafe { *host };
        if host.call.is_none() || host.release.is_none() {
            return Err("incomplete Host ABI".into());
        }
        let logic = Box::new(Echo {
            host: NativeHost(host),
        });
        unsafe {
            *instance = Box::into_raw(logic).cast();
        }
        Ok(String::new())
    })
}

unsafe extern "C" fn dispatch(
    instance: *mut c_void,
    operation: ByteView,
    payload: ByteView,
    session: ByteView,
    workspace: ByteView,
    out: *mut OwnedBytes,
) -> i32 {
    boundary(out, || {
        let logic =
            unsafe { instance.cast::<Echo>().as_ref() }.ok_or("invalid extension instance")?;
        // session / workspace も ABI 境界の検証を通す。
        let _ = unsafe { read_view(&session)? };
        let _ = unsafe { read_view(&workspace)? };
        logic.dispatch(unsafe { read_view(&operation)? }, unsafe {
            read_view(&payload)?
        })
    })
}

unsafe extern "C" fn deactivate(instance: *mut c_void, out: *mut OwnedBytes) -> i32 {
    boundary(out, || {
        if instance.is_null() {
            return Err("invalid extension instance".into());
        }
        drop(unsafe { Box::from_raw(instance.cast::<Echo>()) });
        Ok(String::new())
    })
}

unsafe extern "C" fn release(value: OwnedBytes) {
    if !value.ptr.is_null() && value.len != 0 {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(value.ptr, value.len)) });
    }
}

static API: ExtensionApiV1 = ExtensionApiV1 {
    abi_version: ABI_VERSION,
    struct_size: std::mem::size_of::<ExtensionApiV1>() as u32,
    activate: Some(activate),
    dispatch: Some(dispatch),
    deactivate: Some(deactivate),
    release: Some(release),
};

#[unsafe(no_mangle)]
pub extern "C" fn lapis_extension_v1() -> *const ExtensionApiV1 {
    &API
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn host_call(
        context: *mut c_void,
        method: ByteView,
        payload: ByteView,
        out: *mut OwnedBytes,
    ) -> i32 {
        let calls = unsafe { &mut *context.cast::<Vec<String>>() };
        calls.push(unsafe { read_view(&method) }.unwrap().to_owned());
        let payload: Value = serde_json::from_str(unsafe { read_view(&payload) }.unwrap()).unwrap();
        return_bytes(serde_json::to_vec(&payload).unwrap(), out);
        STATUS_OK
    }

    unsafe extern "C" fn host_release(_: *mut c_void, value: OwnedBytes) {
        unsafe { release(value) };
    }

    unsafe fn take(value: OwnedBytes) -> String {
        let text = String::from_utf8(unsafe { copy_output(value) }.unwrap()).unwrap();
        unsafe { release(value) };
        text
    }

    #[test]
    fn abi_v1_echoes_through_the_supplied_host() {
        let mut calls = Vec::<String>::new();
        let mut host = HostApiV1 {
            abi_version: ABI_VERSION,
            struct_size: std::mem::size_of::<HostApiV1>() as u32,
            context: (&mut calls as *mut Vec<String>).cast(),
            call: Some(host_call),
            release: Some(host_release),
        };
        let api = unsafe { &*lapis_extension_v1() };
        let mut instance = std::ptr::null_mut();
        let mut out = OwnedBytes::EMPTY;
        host.abi_version = 2;
        assert_eq!(
            unsafe { api.activate.unwrap()(&host, &mut instance, &mut out) },
            STATUS_ERROR
        );
        assert!(instance.is_null());
        assert!(unsafe { take(out) }.contains("incompatible"));
        host.abi_version = ABI_VERSION;
        assert_eq!(
            unsafe { api.activate.unwrap()(&host, &mut instance, &mut out) },
            STATUS_OK
        );
        assert_eq!(unsafe { take(out) }, "");
        let operation = ByteView::from_bytes(b"echo.v1.echo");
        let payload = ByteView::from_bytes(br#"{"value":"hello"}"#);
        let session = ByteView::from_bytes(b"session");
        let workspace = ByteView::from_bytes(b"workspace");
        assert_eq!(
            unsafe {
                api.dispatch.unwrap()(instance, operation, payload, session, workspace, &mut out)
            },
            STATUS_OK
        );
        let result: Value = serde_json::from_str(&unsafe { take(out) }).unwrap();
        assert_eq!(result["value"], "hello");
        let unknown = ByteView::from_bytes(b"pty.v1.spawn");
        assert_eq!(
            unsafe {
                api.dispatch.unwrap()(instance, unknown, payload, session, workspace, &mut out)
            },
            STATUS_ERROR
        );
        assert!(unsafe { take(out) }.contains("unknown operation"));
        assert_eq!(
            unsafe { api.deactivate.unwrap()(instance, &mut out) },
            STATUS_OK
        );
        assert_eq!(unsafe { take(out) }, "");
        assert_eq!(calls, ["echo.v1.echo"]);
    }
}

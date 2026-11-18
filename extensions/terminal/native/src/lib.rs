//! Terminal ロジックを公開された native ABI v1 に接続する。

use extension_native_api::{
    ABI_VERSION, ByteView, ExtensionApiV1, HostApiV1, MAX_MESSAGE_BYTES, OwnedBytes, STATUS_ERROR,
    STATUS_OK,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};
use terminal_logic::{TerminalHost, TerminalLogic, process_info, sessions};

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
    fn empty(&self, method: &str, value: Value) -> Result<(), String> {
        self.call(method, value).map(|_| ())
    }
}

impl TerminalHost for NativeHost {
    fn information(&self) -> Result<process_info::Information, String> {
        let value = self.call("process.info.v1.get", Value::Null)?;
        let pty = match value.get("pty").and_then(Value::as_str) {
            Some("conpty") => process_info::PtyFlavor::Conpty,
            Some("posix") => process_info::PtyFlavor::Posix,
            _ => return Err("invalid Host process information".into()),
        };
        let command = value
            .get("default_shell")
            .ok_or("invalid Host process information")?;
        let program = command
            .get("program")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or("invalid Host process information")?
            .to_owned();
        let arguments: Vec<String> = serde_json::from_value(
            command
                .get("arguments")
                .cloned()
                .ok_or("invalid Host process information")?,
        )
        .map_err(|_| "invalid Host process information")?;
        if arguments.len() > 64 || arguments.iter().any(|value| value.len() > 4096) {
            return Err("invalid Host process information".into());
        }
        Ok(process_info::Information {
            pty,
            default_shell: process_info::Command { program, arguments },
        })
    }
    fn spawn(&self, command: &sessions::SpawnRequest) -> Result<String, String> {
        let mut env = BTreeMap::new();
        for entry in &command.environment {
            if env.insert(&entry.name, &entry.value).is_some() {
                return Err("duplicate environment name".into());
            }
        }
        serde_json::from_value(self.call("pty.v1.spawn", json!({"program":command.program,"args":command.arguments,"env":env,"cwd":command.working_directory,"size":{"columns":command.initial_size.columns,"rows":command.initial_size.rows}}))?).map_err(|_| "invalid Host result".into())
    }
    fn attach(&self, id: &str) -> Result<(), String> {
        self.empty("pty.v1.attach", json!({"id":id}))
    }
    fn detach(&self, id: &str) -> Result<(), String> {
        self.empty("pty.v1.detach", json!({"id":id}))
    }
    fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.empty("pty.v1.write", json!({"id":id,"bytes":bytes}))
    }
    fn resize(&self, id: &str, size: sessions::Size) -> Result<(), String> {
        self.empty(
            "pty.v1.resize",
            json!({"id":id,"columns":size.columns,"rows":size.rows}),
        )
    }
    fn read(
        &self,
        id: &str,
        offset: u64,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<sessions::Output, String> {
        let value = self.call(
            "pty.v1.read",
            json!({"id":id,"offset":offset,"max_bytes":max_bytes,"timeout_ms":timeout_ms}),
        )?;
        let bytes =
            serde_json::from_value(value.get("bytes").cloned().ok_or("invalid Host result")?)
                .map_err(|_| "invalid Host result")?;
        let next = value
            .get("next")
            .and_then(Value::as_u64)
            .ok_or("invalid Host result")?;
        let lost_before = serde_json::from_value(
            value
                .get("lost_before")
                .cloned()
                .ok_or("invalid Host result")?,
        )
        .map_err(|_| "invalid Host result")?;
        let exit = match value.get("exited").ok_or("invalid Host result")? {
            Value::Null => None,
            value => Some(sessions::ExitStatus {
                code: serde_json::from_value(
                    value.get("code").cloned().ok_or("invalid Host result")?,
                )
                .map_err(|_| "invalid Host result")?,
            }),
        };
        let read_error = serde_json::from_value(
            value
                .get("read_error")
                .cloned()
                .ok_or("invalid Host result")?,
        )
        .map_err(|_| "invalid Host result")?;
        let write_error = serde_json::from_value(
            value
                .get("write_error")
                .cloned()
                .ok_or("invalid Host result")?,
        )
        .map_err(|_| "invalid Host result")?;
        Ok(sessions::Output {
            bytes,
            next,
            lost_before,
            exit,
            read_error,
            write_error,
        })
    }
    fn kill(&self, id: &str) -> Result<(), String> {
        self.empty("pty.v1.kill", json!({"id":id}))
    }
    fn forget(&self, id: &str) -> Result<(), String> {
        self.empty("pty.v1.forget", json!({"id":id}))
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
        let logic = Box::new(TerminalLogic::new(Box::new(NativeHost(host))));
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
        let logic = unsafe { instance.cast::<TerminalLogic>().as_mut() }
            .ok_or("invalid extension instance")?;
        logic.dispatch(
            unsafe { read_view(&operation)? },
            unsafe { read_view(&payload)? },
            unsafe { read_view(&session)? },
            unsafe { read_view(&workspace)? },
        )
    })
}

unsafe extern "C" fn deactivate(instance: *mut c_void, out: *mut OwnedBytes) -> i32 {
    boundary(out, || {
        if instance.is_null() {
            return Err("invalid extension instance".into());
        }
        let mut logic = unsafe { Box::from_raw(instance.cast::<TerminalLogic>()) };
        logic.deactivate();
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
        _: ByteView,
        out: *mut OwnedBytes,
    ) -> i32 {
        let calls = unsafe { &mut *context.cast::<Vec<String>>() };
        let method = unsafe { read_view(&method) }.unwrap();
        calls.push(method.to_owned());
        let response = match method {
            "process.info.v1.get" => {
                json!({"pty":"conpty","default_shell":{"program":"cmd.exe","arguments":["/D"]}})
            }
            "pty.v1.spawn" => json!("pty-1"),
            "pty.v1.attach" | "pty.v1.kill" | "pty.v1.detach" | "pty.v1.forget" => Value::Null,
            _ => panic!("unexpected Host call"),
        };
        return_bytes(serde_json::to_vec(&response).unwrap(), out);
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
    fn abi_v1_dispatches_to_supplied_host_and_rejects_wrong_versions() {
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
        let operation = ByteView::from_bytes(b"terminal.v1.start-profile");
        let payload = ByteView::from_bytes(
            br#"{"profile":null,"working_directory":".","columns":80,"rows":24}"#,
        );
        let session = ByteView::from_bytes(b"session");
        let workspace = ByteView::from_bytes(b"workspace");
        assert_eq!(
            unsafe {
                api.dispatch.unwrap()(instance, operation, payload, session, workspace, &mut out)
            },
            STATUS_OK
        );
        let result: Value = serde_json::from_str(&unsafe { take(out) }).unwrap();
        assert_eq!(result["id"], "pty-1");
        let bad = ByteView {
            ptr: std::ptr::null(),
            len: 1,
        };
        assert_eq!(
            unsafe { api.dispatch.unwrap()(instance, bad, payload, session, workspace, &mut out) },
            STATUS_ERROR
        );
        assert!(unsafe { take(out) }.contains("invalid ABI input"));
        assert_eq!(
            unsafe { api.deactivate.unwrap()(instance, &mut out) },
            STATUS_OK
        );
        assert_eq!(unsafe { take(out) }, "");
        assert_eq!(
            calls,
            [
                "process.info.v1.get",
                "pty.v1.spawn",
                "pty.v1.attach",
                "pty.v1.kill",
                "pty.v1.detach"
            ]
        );
    }
}

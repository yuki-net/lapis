//! 独立した拡張ホストプロセス内で component を実行する。
//! component の信頼と導入状態は呼び出し元が検証する。

#[cfg(feature = "worker")]
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(feature = "worker")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
#[cfg(feature = "worker")]
use wasmtime::{
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder,
    component::{Component, Instance, Linker, ResourceTable},
};
#[cfg(feature = "worker")]
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

#[cfg(feature = "worker")]
pub mod native;
pub mod supervisor;

#[cfg(feature = "worker")]
#[allow(dead_code)]
mod pty_bindings {
    wasmtime::component::bindgen!({
        path: "../../../contracts/extension-api/wit/pty-v1",
        world: "pty-host",
    });
}
#[cfg(feature = "worker")]
#[allow(dead_code)]
mod process_info_bindings {
    wasmtime::component::bindgen!({
        path: "../../../contracts/extension-api/wit/process-info-v1",
        world: "process-info-host",
    });
}
#[cfg(feature = "worker")]
mod pty_worker;

/// 双方向の能力呼び出し。操作名と payload は能力契約側が定める。
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostCall {
    pub kind: String,
    pub id: u64,
    pub extension: String,
    pub method: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostReply {
    pub kind: String,
    pub id: u64,
    pub result: Result<serde_json::Value, String>,
}

/// manifest の package 契約とは別の component ライフサイクル契約。
pub const LIFECYCLE_CONTRACT: &str = "lapis.extension.lifecycle";
pub const LIFECYCLE_VERSION: &str = "1.0.0";
pub const PTY_VERSION: &str = "1.0.0";
pub const PROCESS_INFO_CONTRACT: &str = "lapis.process.info";
pub const PROCESS_INFO_VERSION: &str = "1.0.0";
pub const DISPATCH_CONTRACT: &str = "lapis.extension.dispatch";
pub const DISPATCH_VERSION: &str = "1.0.0";
pub const MAX_DISPATCH_BYTES: usize = 64 * 1024;
/// Host のみが送信するライフサイクル通知の名前空間。
pub const SYSTEM_OPERATION_PREFIX: &str = "lapis.system.";
pub const SESSION_DETACHED_OPERATION: &str = "lapis.system.session-detached";
pub const RELEASE_CLAIMS_OPERATION: &str = "lapis.system.release-claims";

/// Host が認証済み接続から生成する。Client の payload からは受け取らない。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationContext {
    pub session: String,
    pub workspace: String,
}

impl InvocationContext {
    pub fn valid(&self) -> bool {
        !self.session.is_empty()
            && self.session.len() <= 256
            && !self.workspace.is_empty()
            && self.workspace.len() <= 256
    }
}

#[cfg(feature = "worker")]
const MAX_COMPONENT_BYTES: usize = 8 * 1024 * 1024;
#[cfg(feature = "worker")]
const MAX_LINE_BYTES: u64 = 12 * 1024 * 1024;
#[cfg(feature = "worker")]
const MAX_EXTENSIONS: usize = 64;
#[cfg(feature = "worker")]
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
#[cfg(feature = "worker")]
const CALL_FUEL: u64 = 10_000_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Handshake,
    Activate {
        id: String,
        component: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        require_dispatch: bool,
    },
    Deactivate {
        id: String,
    },
    Dispatch {
        id: String,
        operation: String,
        payload: String,
        context: InvocationContext,
    },
    Shutdown,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_info_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[cfg(feature = "worker")]
struct WorkerState {
    limits: StoreLimits,
    wasi: WasiCtx,
    table: ResourceTable,
    extension: String,
    broker: Option<Broker>,
}

#[cfg(feature = "worker")]
type Broker =
    Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;

#[cfg(feature = "worker")]
impl WasiView for WorkerState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

#[cfg(feature = "worker")]
struct Loaded {
    store: Store<WorkerState>,
    instance: Instance,
}

#[cfg(feature = "worker")]
enum LoadedExtension {
    Component(Loaded),
    Native(native::NativeExtension),
}

/// Host が検証した拡張を同じプロセス内で管理する。
#[cfg(feature = "worker")]
pub struct ExtensionRuntime {
    engine: Engine,
    loaded: BTreeMap<String, LoadedExtension>,
    broker: Option<Broker>,
}

#[cfg(feature = "worker")]
impl ExtensionRuntime {
    pub fn new() -> Result<Self, String> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|error| error.to_string())?;
        Ok(Self {
            engine,
            loaded: BTreeMap::new(),
            broker: None,
        })
    }

    pub fn activate(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.activate_with_contracts(id, bytes, false)
    }

    fn validate_new_id(&self, id: &str) -> Result<(), String> {
        if id.is_empty()
            || id.len() > 128
            || !id.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_')
            })
        {
            return Err("invalid extension id".into());
        }
        if self.loaded.contains_key(id) {
            return Err("extension is already active".into());
        }
        if self.loaded.len() >= MAX_EXTENSIONS {
            return Err("extension limit exceeded".into());
        }
        Ok(())
    }

    /// 検証済みの native 拡張を component と同じ目録で管理する。
    ///
    /// # Safety
    /// 呼び出し側は `NativeExtension::load_verified` の安全条件を満たすこと。
    pub unsafe fn activate_native_verified(
        &mut self,
        id: &str,
        path: &std::path::Path,
        broker: native::NativeBroker,
    ) -> Result<(), String> {
        self.validate_new_id(id)?;
        let extension = unsafe { native::NativeExtension::load_verified(path, broker) }?;
        self.loaded
            .insert(id.to_owned(), LoadedExtension::Native(extension));
        Ok(())
    }

    fn activate_with_contracts(
        &mut self,
        id: &str,
        bytes: &[u8],
        require_dispatch: bool,
    ) -> Result<(), String> {
        self.validate_new_id(id)?;
        if bytes.len() > MAX_COMPONENT_BYTES {
            return Err("extension limit exceeded".into());
        }
        let component = Component::new(&self.engine, bytes).map_err(|error| error.to_string())?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .memories(4)
            .instances(32)
            .tables(32)
            .table_elements(100_000)
            .build();
        // WASI の既定値は環境変数・ファイル・ネットワークを公開しない。
        let state = WorkerState {
            limits,
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            extension: id.to_owned(),
            broker: self.broker.clone(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(CALL_FUEL)
            .map_err(|error| error.to_string())?;
        let mut linker = Linker::<WorkerState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|error| error.to_string())?;
        pty_bindings::lapis::pty::sessions::add_to_linker::<
            WorkerState,
            wasmtime::component::HasSelf<WorkerState>,
        >(&mut linker, |state| state)
        .map_err(|error| error.to_string())?;
        process_info_bindings::lapis::process_info::host::add_to_linker::<
            WorkerState,
            wasmtime::component::HasSelf<WorkerState>,
        >(&mut linker, |state| state)
        .map_err(|error| error.to_string())?;
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|error| error.to_string())?;
        // 実行前に停止用 export も含めて契約を確認する。
        instance
            .get_typed_func::<(), ()>(&mut store, "deactivate")
            .map_err(|error| error.to_string())?;
        if require_dispatch {
            instance
                .get_typed_func::<(&str, &str, &str, &str), (Result<String, String>,)>(
                    &mut store, "dispatch",
                )
                .map_err(|_| "extension does not implement dispatch v1".to_owned())?;
        }
        let activate = instance
            .get_typed_func::<(), ()>(&mut store, "activate")
            .map_err(|error| error.to_string())?;
        activate
            .call(&mut store, ())
            .map_err(|error| error.to_string())?;
        activate
            .post_return(&mut store)
            .map_err(|error| error.to_string())?;
        self.loaded.insert(
            id.to_owned(),
            LoadedExtension::Component(Loaded { store, instance }),
        );
        Ok(())
    }

    pub fn deactivate(&mut self, id: &str) -> Result<(), String> {
        let loaded = self
            .loaded
            .remove(id)
            .ok_or_else(|| "extension is not active".to_owned())?;
        let mut loaded = match loaded {
            LoadedExtension::Native(mut extension) => return extension.deactivate(),
            LoadedExtension::Component(component) => component,
        };
        loaded
            .store
            .set_fuel(CALL_FUEL)
            .map_err(|error| error.to_string())?;
        let func = loaded
            .instance
            .get_typed_func::<(), ()>(&mut loaded.store, "deactivate")
            .map_err(|error| error.to_string())?;
        func.call(&mut loaded.store, ())
            .map_err(|error| error.to_string())?;
        func.post_return(&mut loaded.store)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 拡張操作を実行する。payload の検証と操作権限は呼び出し元の契約が担う。
    pub fn dispatch(
        &mut self,
        id: &str,
        operation: &str,
        payload: &str,
        context: &InvocationContext,
    ) -> Result<String, String> {
        if !context.valid()
            || operation.is_empty()
            || operation.len() > 256
            || payload.len() > MAX_DISPATCH_BYTES
        {
            return Err("invalid dispatch request".into());
        }
        let loaded = self.loaded.get_mut(id).ok_or("extension is not active")?;
        let loaded = match loaded {
            LoadedExtension::Native(extension) => {
                return extension.dispatch(operation, payload, context);
            }
            LoadedExtension::Component(component) => component,
        };
        loaded
            .store
            .set_fuel(CALL_FUEL)
            .map_err(|error| error.to_string())?;
        let function = loaded
            .instance
            .get_typed_func::<(&str, &str, &str, &str), (Result<String, String>,)>(
                &mut loaded.store,
                "dispatch",
            )
            .map_err(|_| "extension does not support dispatch v1".to_owned())?;
        let (result,) = function
            .call(
                &mut loaded.store,
                (operation, payload, &context.session, &context.workspace),
            )
            .map_err(|error| error.to_string())?;
        function
            .post_return(&mut loaded.store)
            .map_err(|error| error.to_string())?;
        if result
            .as_ref()
            .map_or_else(|error| error.len(), |value| value.len())
            > MAX_DISPATCH_BYTES
        {
            return Err("dispatch response too large".into());
        }
        result
    }

    pub fn handle(&mut self, request: Request) -> Response {
        if matches!(request, Request::Handshake) {
            return Response {
                ok: true,
                error: None,
                lifecycle_version: Some(LIFECYCLE_VERSION.to_owned()),
                pty_version: Some(PTY_VERSION.to_owned()),
                process_info_version: Some(PROCESS_INFO_VERSION.to_owned()),
                dispatch_version: Some(DISPATCH_VERSION.to_owned()),
                result: None,
            };
        }
        if let Request::Dispatch {
            id,
            operation,
            payload,
            context,
        } = request
        {
            return match self.dispatch(&id, &operation, &payload, &context) {
                Ok(result) => Response {
                    ok: true,
                    error: None,
                    lifecycle_version: None,
                    pty_version: None,
                    process_info_version: None,
                    dispatch_version: None,
                    result: Some(result),
                },
                Err(error) => Response {
                    ok: false,
                    error: Some(error),
                    lifecycle_version: None,
                    pty_version: None,
                    process_info_version: None,
                    dispatch_version: None,
                    result: None,
                },
            };
        }
        let result = match request {
            Request::Handshake | Request::Dispatch { .. } => unreachable!(),
            Request::Activate {
                id,
                component,
                require_dispatch,
            } => {
                if component.len() > MAX_COMPONENT_BYTES.div_ceil(3) * 4 {
                    Err("component limit exceeded".into())
                } else {
                    STANDARD
                        .decode(component)
                        .map_err(|_| "invalid component encoding".to_owned())
                        .and_then(|bytes| {
                            self.activate_with_contracts(&id, &bytes, require_dispatch)
                        })
                }
            }
            Request::Deactivate { id } => self.deactivate(&id),
            Request::Shutdown => {
                let ids: Vec<_> = self.loaded.keys().cloned().collect();
                let mut failure = None;
                for id in ids {
                    if let Err(error) = self.deactivate(&id) {
                        failure.get_or_insert(error);
                    }
                }
                failure.map_or(Ok(()), Err)
            }
        };
        match result {
            Ok(()) => Response {
                ok: true,
                error: None,
                lifecycle_version: None,
                pty_version: None,
                process_info_version: None,
                dispatch_version: None,
                result: None,
            },
            Err(error) => Response {
                ok: false,
                error: Some(error),
                lifecycle_version: None,
                pty_version: None,
                process_info_version: None,
                dispatch_version: None,
                result: None,
            },
        }
    }
}

/// 1 行 1 request / response。長すぎる入力は処理せず接続を閉じる。
#[cfg(feature = "worker")]
pub fn serve(
    input: impl BufRead + Send + 'static,
    output: impl Write + Send + 'static,
) -> io::Result<()> {
    let input = Arc::new(Mutex::new(input));
    let output = Arc::new(Mutex::new(output));
    let next_id = AtomicU64::new(1);
    let broker_input = Arc::clone(&input);
    let broker_output = Arc::clone(&output);
    let broker: Broker = Arc::new(move |extension, method, payload| {
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        let call = HostCall {
            kind: "call".into(),
            id,
            extension: extension.into(),
            method: method.into(),
            payload,
        };
        {
            let mut output = broker_output
                .lock()
                .map_err(|_| "能力応答を送信できません".to_owned())?;
            serde_json::to_writer(&mut *output, &call).map_err(|e| e.to_string())?;
            output
                .write_all(b"\n")
                .and_then(|_| output.flush())
                .map_err(|e| e.to_string())?;
        }
        let mut line = Vec::new();
        let mut input = broker_input
            .lock()
            .map_err(|_| "能力応答を取得できません".to_owned())?;
        let read = std::io::Read::take(&mut *input, MAX_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if read == 0 || read as u64 > MAX_LINE_BYTES {
            return Err("能力応答が不正です".into());
        }
        let reply: HostReply =
            serde_json::from_slice(&line).map_err(|_| "能力応答が不正です".to_owned())?;
        if reply.kind != "reply" || reply.id != id {
            return Err("能力応答が一致しません".into());
        }
        reply.result
    });
    let mut runtime = ExtensionRuntime::new().map_err(io::Error::other)?;
    runtime.broker = Some(broker);
    loop {
        let mut line = Vec::new();
        let read = std::io::Read::take(
            &mut *input
                .lock()
                .map_err(|_| io::Error::other("入力 lock 失敗"))?,
            MAX_LINE_BYTES + 1,
        )
        .read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if read as u64 > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
        let (response, shutdown) = match serde_json::from_slice::<Request>(&line) {
            Ok(request) => {
                let shutdown = matches!(request, Request::Shutdown);
                (runtime.handle(request), shutdown)
            }
            Err(_) => (
                Response {
                    ok: false,
                    error: Some("invalid request".into()),
                    lifecycle_version: None,
                    pty_version: None,
                    process_info_version: None,
                    dispatch_version: None,
                    result: None,
                },
                false,
            ),
        };
        let mut output = output
            .lock()
            .map_err(|_| io::Error::other("出力 lock 失敗"))?;
        serde_json::to_writer(&mut *output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
        if shutdown {
            break;
        }
    }
    Ok(())
}

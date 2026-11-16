//! 独立した拡張ホストプロセス内で component を実行する。
//! component の信頼と導入状態は呼び出し元が検証する。

#[cfg(feature = "worker")]
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
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

pub mod supervisor;

/// manifest の package 契約とは別の component ライフサイクル契約。
pub const LIFECYCLE_CONTRACT: &str = "lapis.extension.lifecycle";
pub const LIFECYCLE_VERSION: &str = "1.0.0";

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
    Activate { id: String, component: String },
    Deactivate { id: String },
    Shutdown,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_version: Option<String>,
}

#[cfg(feature = "worker")]
struct WorkerState {
    limits: StoreLimits,
    wasi: WasiCtx,
    table: ResourceTable,
}

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

/// Host が署名・互換性を確認した component を受け取り、同じプロセス内で管理する。
#[cfg(feature = "worker")]
pub struct ExtensionRuntime {
    engine: Engine,
    loaded: BTreeMap<String, Loaded>,
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
        })
    }

    pub fn activate(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
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
        if self.loaded.len() >= MAX_EXTENSIONS || bytes.len() > MAX_COMPONENT_BYTES {
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
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(CALL_FUEL)
            .map_err(|error| error.to_string())?;
        let mut linker = Linker::<WorkerState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|error| error.to_string())?;
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|error| error.to_string())?;
        // 実行前に停止用 export も含めて契約を確認する。
        instance
            .get_typed_func::<(), ()>(&mut store, "deactivate")
            .map_err(|error| error.to_string())?;
        let activate = instance
            .get_typed_func::<(), ()>(&mut store, "activate")
            .map_err(|error| error.to_string())?;
        activate
            .call(&mut store, ())
            .map_err(|error| error.to_string())?;
        activate
            .post_return(&mut store)
            .map_err(|error| error.to_string())?;
        self.loaded
            .insert(id.to_owned(), Loaded { store, instance });
        Ok(())
    }

    pub fn deactivate(&mut self, id: &str) -> Result<(), String> {
        let mut loaded = self
            .loaded
            .remove(id)
            .ok_or_else(|| "extension is not active".to_owned())?;
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

    pub fn handle(&mut self, request: Request) -> Response {
        if matches!(request, Request::Handshake) {
            return Response {
                ok: true,
                error: None,
                lifecycle_version: Some(LIFECYCLE_VERSION.to_owned()),
            };
        }
        let result = match request {
            Request::Handshake => unreachable!(),
            Request::Activate { id, component } => {
                if component.len() > MAX_COMPONENT_BYTES.div_ceil(3) * 4 {
                    Err("component limit exceeded".into())
                } else {
                    STANDARD
                        .decode(component)
                        .map_err(|_| "invalid component encoding".to_owned())
                        .and_then(|bytes| self.activate(&id, &bytes))
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
            },
            Err(error) => Response {
                ok: false,
                error: Some(error),
                lifecycle_version: None,
            },
        }
    }
}

/// 1 行 1 request / response。長すぎる入力は処理せず接続を閉じる。
#[cfg(feature = "worker")]
pub fn serve(input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut input = input;
    let mut runtime = ExtensionRuntime::new().map_err(io::Error::other)?;
    loop {
        let mut line = Vec::new();
        let read =
            std::io::Read::take(&mut input, MAX_LINE_BYTES + 1).read_until(b'\n', &mut line)?;
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
                },
                false,
            ),
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
        if shutdown {
            break;
        }
    }
    Ok(())
}

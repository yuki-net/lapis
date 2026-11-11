//! 全スコープの設定値を1か所で所有し、変更を購読できるようにする。

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use settings::{
    DefinitionCatalog, ResolvedSettings, SettingKey, SettingValue, SettingsContext,
    SettingsDocument, SettingsError, SettingsRepository, SettingsScope, catalog,
};

struct Owner {
    repository: Arc<dyn SettingsRepository>,
    document: SettingsDocument,
}

struct Inner {
    context: SettingsContext,
    owners: BTreeMap<SettingsScope, Owner>,
    subscribers: Vec<mpsc::Sender<ResolvedSettings>>,
}

/// 設定値の共有 state。書き込みは所有者の repository を通し、成功後だけ購読者へ通知する。
pub struct SettingsModel {
    catalog: &'static DefinitionCatalog,
    /// 書き込みを直列化し、保存順と通知順を一致させる。
    writer: Mutex<()>,
    inner: Mutex<Inner>,
}

impl SettingsModel {
    pub fn new(context: SettingsContext) -> Self {
        Self::with_catalog(catalog(), context)
    }

    pub fn with_catalog(catalog: &'static DefinitionCatalog, context: SettingsContext) -> Self {
        Self {
            catalog,
            writer: Mutex::new(()),
            inner: Mutex::new(Inner {
                context,
                owners: BTreeMap::new(),
                subscribers: Vec::new(),
            }),
        }
    }

    pub fn catalog(&self) -> &'static DefinitionCatalog {
        self.catalog
    }

    pub fn context(&self) -> SettingsContext {
        self.inner.lock().unwrap().context.clone()
    }

    /// スコープの保存先を登録し、保存済みの値を読み込む。
    pub fn attach(
        &self,
        scope: SettingsScope,
        repository: Arc<dyn SettingsRepository>,
    ) -> Result<(), SettingsError> {
        let _writer = self.writer.lock().unwrap();
        let document = repository.load()?;
        if document.scope() != &scope {
            return Err(SettingsError::new(format!(
                "保存された設定のスコープが一致しません: {:?}",
                document.scope()
            )));
        }
        document.validate(self.catalog)?;
        self.inner.lock().unwrap().owners.insert(
            scope,
            Owner {
                repository,
                document,
            },
        );
        self.publish();
        Ok(())
    }

    pub fn scopes(&self) -> Vec<SettingsScope> {
        self.inner.lock().unwrap().owners.keys().cloned().collect()
    }

    /// 保存済みの文書。保存前の表示に使う。
    pub fn document(&self, scope: &SettingsScope) -> Option<SettingsDocument> {
        self.inner
            .lock()
            .unwrap()
            .owners
            .get(scope)
            .map(|owner| owner.document.clone())
    }

    /// 現在のコンテキストに対する実効値。
    pub fn resolved(&self) -> ResolvedSettings {
        let inner = self.inner.lock().unwrap();
        let documents = inner
            .owners
            .values()
            .map(|owner| &owner.document)
            .collect::<Vec<_>>();
        ResolvedSettings::resolve(&inner.context, self.catalog, &documents)
    }

    /// 値を保存する。保存に失敗した場合は state を変えない。
    pub fn set(
        &self,
        scope: SettingsScope,
        key: SettingKey,
        value: SettingValue,
    ) -> Result<(), SettingsError> {
        self.apply(scope, key, Some(value))
    }

    /// 保存値を削除し、既定値に戻す。
    pub fn clear(&self, scope: SettingsScope, key: SettingKey) -> Result<(), SettingsError> {
        self.apply(scope, key, None)
    }

    fn apply(
        &self,
        scope: SettingsScope,
        key: SettingKey,
        value: Option<SettingValue>,
    ) -> Result<(), SettingsError> {
        self.update(scope, &[(key, value)])
    }

    /// 複数の設定を1回の保存でまとめて更新する。`None` は既定値へ戻す。
    pub fn update(
        &self,
        scope: SettingsScope,
        values: &[(SettingKey, Option<SettingValue>)],
    ) -> Result<(), SettingsError> {
        for (key, value) in values {
            let definition = self.catalog.definition(*key).ok_or_else(|| {
                SettingsError::new(format!("未知の設定キーです: {}", key.as_str()))
            })?;
            if !definition.allows_scope(&scope) {
                return Err(SettingsError::new(format!(
                    "このスコープでは保存できない設定です: {}",
                    key.as_str()
                )));
            }
            if let Some(value) = value
                && !definition.accepts(value)
            {
                return Err(SettingsError::new(format!(
                    "設定値が定義と一致しません: {}",
                    key.as_str()
                )));
            }
        }

        let _writer = self.writer.lock().unwrap();
        // 保存中は state を触らない。遅い I/O の間も読み取りは待たせない。
        let (repository, document) = {
            let inner = self.inner.lock().unwrap();
            let owner = inner.owners.get(&scope).ok_or_else(|| {
                SettingsError::new(format!("設定の保存先がありません: {scope:?}"))
            })?;
            let mut document = owner.document.clone();
            let mut changed = false;
            for (key, value) in values {
                if document.get(*key) == value.as_ref() {
                    continue;
                }
                changed = true;
                match value {
                    Some(value) => document.insert(*key, value.clone()),
                    None => {
                        document.remove(*key);
                    }
                }
            }
            if !changed {
                return Ok(());
            }
            (owner.repository.clone(), document)
        };
        repository.save(&document)?;
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(owner) = inner.owners.get_mut(&scope) {
                owner.document = document;
            }
        }
        self.publish();
        Ok(())
    }

    /// 変更の購読を開始する。購読中は最新の実効値だけを順に受け取る。
    pub fn subscribe(&self) -> SettingsWatch {
        let (sender, receiver) = mpsc::channel();
        self.inner.lock().unwrap().subscribers.push(sender);
        SettingsWatch(receiver)
    }

    fn publish(&self) {
        let resolved = self.resolved();
        let mut inner = self.inner.lock().unwrap();
        inner
            .subscribers
            .retain(|subscriber| subscriber.send(resolved.clone()).is_ok());
    }
}

/// 設定変更の購読ハンドル。
pub struct SettingsWatch(Receiver<ResolvedSettings>);

impl SettingsWatch {
    pub fn try_recv(&self) -> Result<ResolvedSettings, TryRecvError> {
        self.0.try_recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<ResolvedSettings, RecvTimeoutError> {
        self.0.recv_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::{HostId, ProjectId};
    use settings::{TERMINAL_FONT_SIZE, UI_THEME};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct MemoryRepository {
        document: Mutex<SettingsDocument>,
        fail: AtomicBool,
        saves: AtomicUsize,
        on_save: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl MemoryRepository {
        fn new(scope: SettingsScope) -> Self {
            Self {
                document: Mutex::new(SettingsDocument::empty(scope)),
                fail: AtomicBool::new(false),
                saves: AtomicUsize::new(0),
                on_save: None,
            }
        }

        fn failing(scope: SettingsScope) -> Self {
            let repository = Self::new(scope);
            repository.fail.store(true, Ordering::SeqCst);
            repository
        }
    }

    impl SettingsRepository for MemoryRepository {
        fn load(&self) -> Result<SettingsDocument, SettingsError> {
            Ok(self.document.lock().unwrap().clone())
        }

        fn save(&self, document: &SettingsDocument) -> Result<(), SettingsError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(SettingsError::new("settings storage unavailable"));
            }
            if let Some(callback) = &self.on_save {
                callback();
            }
            self.saves.fetch_add(1, Ordering::SeqCst);
            *self.document.lock().unwrap() = document.clone();
            Ok(())
        }
    }

    fn model(context: SettingsContext) -> (SettingsModel, Arc<MemoryRepository>) {
        let model = SettingsModel::new(context);
        let repository = Arc::new(MemoryRepository::new(SettingsScope::User));
        model
            .attach(SettingsScope::User, repository.clone())
            .unwrap();
        (model, repository)
    }

    #[test]
    fn set_persists_and_notifies_subscribers() {
        let (model, repository) = model(SettingsContext::new());
        let watch = model.subscribe();
        model
            .set(
                SettingsScope::User,
                UI_THEME,
                SettingValue::text("lapis.white"),
            )
            .unwrap();
        let resolved = watch.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(resolved.theme(), "lapis.white");
        assert_eq!(repository.saves.load(Ordering::SeqCst), 1);
        assert_eq!(
            repository.document.lock().unwrap().get(UI_THEME),
            Some(&SettingValue::text("lapis.white"))
        );
    }

    #[test]
    fn failed_save_keeps_the_previous_value() {
        let context = SettingsContext::new();
        let model = SettingsModel::new(context);
        model
            .attach(
                SettingsScope::User,
                Arc::new(MemoryRepository::failing(SettingsScope::User)),
            )
            .unwrap();
        let watch = model.subscribe();
        assert!(
            model
                .set(
                    SettingsScope::User,
                    UI_THEME,
                    SettingValue::text("lapis.white")
                )
                .is_err()
        );
        assert!(watch.try_recv().is_err());
        assert_eq!(model.resolved().theme(), "lapis.dark");
    }

    #[test]
    fn saving_an_unchanged_value_skips_persistence() {
        let (model, repository) = model(SettingsContext::new());
        model
            .set(
                SettingsScope::User,
                TERMINAL_FONT_SIZE,
                SettingValue::integer(14),
            )
            .unwrap();
        model
            .set(
                SettingsScope::User,
                TERMINAL_FONT_SIZE,
                SettingValue::integer(14),
            )
            .unwrap();
        assert_eq!(repository.saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn definitions_restrict_which_scopes_can_store_a_key() {
        let model =
            SettingsModel::new(SettingsContext::new().with_project(ProjectId::new("lapis")));
        model
            .attach(
                SettingsScope::User,
                Arc::new(MemoryRepository::new(SettingsScope::User)),
            )
            .unwrap();
        model
            .attach(
                SettingsScope::ProjectLocal {
                    id: ProjectId::new("lapis"),
                },
                Arc::new(MemoryRepository::new(SettingsScope::ProjectLocal {
                    id: ProjectId::new("lapis"),
                })),
            )
            .unwrap();
        assert!(
            model
                .set(
                    SettingsScope::ProjectLocal {
                        id: ProjectId::new("lapis")
                    },
                    UI_THEME,
                    SettingValue::text("lapis.white"),
                )
                .is_err()
        );
        assert!(
            model
                .set(
                    SettingsScope::ProjectLocal {
                        id: ProjectId::new("lapis")
                    },
                    TERMINAL_FONT_SIZE,
                    SettingValue::integer(16),
                )
                .is_ok()
        );
        assert_eq!(model.resolved().terminal_font_size(), Some(16));
    }

    #[test]
    fn attaching_rejects_a_document_for_another_scope() {
        let model = SettingsModel::new(SettingsContext::new().with_host(HostId::new("host-a")));
        let repository = Arc::new(MemoryRepository::new(SettingsScope::Host {
            id: HostId::new("host-b"),
        }));
        assert!(
            model
                .attach(
                    SettingsScope::Host {
                        id: HostId::new("host-a")
                    },
                    repository
                )
                .is_err()
        );
    }

    #[test]
    fn reads_do_not_wait_for_a_slow_save() {
        struct SlowRepository {
            document: Mutex<SettingsDocument>,
            started: mpsc::Sender<()>,
            release: Mutex<Receiver<()>>,
        }

        impl SettingsRepository for SlowRepository {
            fn load(&self) -> Result<SettingsDocument, SettingsError> {
                Ok(self.document.lock().unwrap().clone())
            }

            fn save(&self, document: &SettingsDocument) -> Result<(), SettingsError> {
                self.started.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                *self.document.lock().unwrap() = document.clone();
                Ok(())
            }
        }

        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let model = Arc::new(SettingsModel::new(SettingsContext::new()));
        model
            .attach(
                SettingsScope::User,
                Arc::new(SlowRepository {
                    document: Mutex::new(SettingsDocument::empty(SettingsScope::User)),
                    started,
                    release: Mutex::new(release_rx),
                }),
            )
            .unwrap();

        let writer_model = model.clone();
        let writer = std::thread::spawn(move || {
            writer_model.set(
                SettingsScope::User,
                UI_THEME,
                SettingValue::text("lapis.white"),
            )
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(model.resolved().theme(), "lapis.dark");
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        assert_eq!(model.resolved().theme(), "lapis.white");
    }
}

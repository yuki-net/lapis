//! 全スコープの設定値を1か所で所有し、変更を購読できるようにする。

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use settings::{
    DefinitionCatalog, ExtensionSettingDefinition, ExtensionSettingsCatalog, ResolvedSettings,
    SettingKey, SettingValue, SettingsContext, SettingsDocument, SettingsError, SettingsRepository,
    SettingsScope, catalog,
};

struct Owner {
    repository: Arc<dyn SettingsRepository>,
    document: SettingsDocument,
}

struct Inner {
    context: SettingsContext,
    extensions: ExtensionSettingsCatalog,
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
                extensions: ExtensionSettingsCatalog::default(),
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

    /// 拡張を有効にしたときに定義を登録し、既存の保存値も検証する。
    pub fn register_extension_settings(
        &self,
        owner: &str,
        definitions: Vec<ExtensionSettingDefinition>,
    ) -> Result<(), SettingsError> {
        let _writer = self.writer.lock().unwrap();
        {
            let mut inner = self.inner.lock().unwrap();
            let mut extensions = inner.extensions.clone();
            extensions.register(owner, definitions)?;
            if extensions.definitions().any(|(key, _)| {
                self.catalog
                    .settings()
                    .iter()
                    .any(|definition| definition.key.as_str() == key)
            }) {
                return Err(SettingsError::new(
                    "拡張設定が既存の設定キーと重複しています",
                ));
            }
            for stored in inner.owners.values() {
                stored
                    .document
                    .validate_with_extensions(self.catalog, &extensions, true)?;
            }
            inner.extensions = extensions;
        }
        self.publish();
        Ok(())
    }

    /// 定義と表示だけを除き、各スコープの保存値は削除しない。
    pub fn unregister_extension_settings(&self, owner: &str) -> bool {
        let _writer = self.writer.lock().unwrap();
        let removed = self.inner.lock().unwrap().extensions.unregister(owner);
        if removed {
            self.publish();
        }
        removed
    }

    /// 現在有効な拡張設定の定義。呼び出し側はスナップショットを得る。
    pub fn extension_settings(&self) -> ExtensionSettingsCatalog {
        self.inner.lock().unwrap().extensions.clone()
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
        document.validate_with_extensions(
            self.catalog,
            &self.inner.lock().unwrap().extensions,
            true,
        )?;
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
        ResolvedSettings::resolve_with_extensions(
            &inner.context,
            self.catalog,
            &inner.extensions,
            &documents,
        )
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
        let entries = values
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>();
        self.persist_updates(scope, &entries)
    }

    /// 現在登録された拡張定義に従って値を保存する。
    pub fn update_extension(
        &self,
        scope: SettingsScope,
        values: &[(&str, Option<SettingValue>)],
    ) -> Result<(), SettingsError> {
        let _writer = self.writer.lock().unwrap();
        {
            let inner = self.inner.lock().unwrap();
            for (key, value) in values {
                let definition = inner
                    .extensions
                    .definition(key)
                    .ok_or_else(|| SettingsError::new(format!("未登録の拡張設定: {key}")))?;
                if !definition.allows_scope(&scope)
                    || value
                        .as_ref()
                        .is_some_and(|value| !definition.accepts(value))
                {
                    return Err(SettingsError::new(format!(
                        "拡張設定の値またはスコープが不正です: {key}"
                    )));
                }
            }
        }
        self.persist_updates(scope, values)
    }

    /// writer lock を保持したまま共通の文書保存経路を使う。
    fn persist_updates(
        &self,
        scope: SettingsScope,
        values: &[(&str, Option<SettingValue>)],
    ) -> Result<(), SettingsError> {
        // 保存中は state を触らない。遅い I/O の間も読み取りは待たせない。
        let (repository, document) = {
            let inner = self.inner.lock().unwrap();
            let owner = inner.owners.get(&scope).ok_or_else(|| {
                SettingsError::new(format!("設定の保存先がありません: {scope:?}"))
            })?;
            let mut document = owner.document.clone();
            let mut changed = false;
            for (key, value) in values {
                if document.values.get(*key) == value.as_ref() {
                    continue;
                }
                changed = true;
                match value {
                    Some(value) => {
                        document.values.insert((*key).to_owned(), value.clone());
                    }
                    None => {
                        document.values.remove(*key);
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

    fn extension_definition() -> ExtensionSettingDefinition {
        ExtensionSettingDefinition {
            name: "font-size".into(),
            value_kind: settings::ValueKind::Integer,
            scopes: vec![settings::ScopeKind::User],
            default: Some(SettingValue::integer(12)),
            allowed_values: None,
        }
    }

    #[test]
    fn extension_settings_are_validated_saved_and_restored_after_disable() {
        let (model, repository) = model(SettingsContext::new());
        let key = "extensions/dev.example.terminal/font-size";
        model
            .register_extension_settings("dev.example.terminal", vec![extension_definition()])
            .unwrap();
        let watch = model.subscribe();
        assert_eq!(
            model.resolved().extension_setting(key).unwrap().value(),
            Some(&SettingValue::integer(12))
        );
        assert!(
            model
                .update_extension(
                    SettingsScope::User,
                    &[(key, Some(SettingValue::text("bad")))]
                )
                .is_err()
        );
        assert!(
            model
                .update_extension(
                    SettingsScope::User,
                    &[("extensions/other/value", Some(SettingValue::integer(14)))]
                )
                .is_err()
        );
        assert!(watch.try_recv().is_err());
        model
            .update_extension(
                SettingsScope::User,
                &[(key, Some(SettingValue::integer(16)))],
            )
            .unwrap();
        assert_eq!(
            watch
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .extension_setting(key)
                .unwrap()
                .value(),
            Some(&SettingValue::integer(16))
        );
        assert_eq!(
            repository.document.lock().unwrap().values.get(key),
            Some(&SettingValue::integer(16))
        );
        assert!(model.unregister_extension_settings("dev.example.terminal"));
        assert!(model.resolved().extension_setting(key).is_none());
        // 無効中の他の設定変更でも拡張の値を消さない。
        model
            .set(
                SettingsScope::User,
                UI_THEME,
                SettingValue::text("lapis.white"),
            )
            .unwrap();
        assert_eq!(
            repository.document.lock().unwrap().values.get(key),
            Some(&SettingValue::integer(16))
        );
        let restarted = SettingsModel::new(SettingsContext::new());
        restarted
            .attach(SettingsScope::User, repository.clone())
            .unwrap();
        assert!(restarted.resolved().extension_setting(key).is_none());
        restarted
            .register_extension_settings("dev.example.terminal", vec![extension_definition()])
            .unwrap();
        assert_eq!(
            restarted.resolved().extension_setting(key).unwrap().value(),
            Some(&SettingValue::integer(16))
        );
        assert_eq!(restarted.resolved().theme(), "lapis.white");
    }

    #[test]
    fn extension_registration_is_atomic_for_invalid_stored_values() {
        let (model, repository) = model(SettingsContext::new());
        let key = "extensions/dev.example.terminal/font-size";
        // 無効な拡張の保存値は壊さず読み、再有効化時に定義と照合する。
        repository
            .document
            .lock()
            .unwrap()
            .values
            .insert(key.into(), SettingValue::text("wrong"));
        model
            .attach(SettingsScope::User, repository.clone())
            .unwrap();
        assert!(
            model
                .register_extension_settings("dev.example.terminal", vec![extension_definition()])
                .is_err()
        );
        assert!(model.resolved().extension_setting(key).is_none());
        assert_eq!(
            repository.document.lock().unwrap().values.get(key),
            Some(&SettingValue::text("wrong"))
        );
        assert_eq!(repository.saves.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn attaching_keeps_rejecting_unknown_non_extension_keys() {
        let repository = Arc::new(MemoryRepository::new(SettingsScope::User));
        repository
            .document
            .lock()
            .unwrap()
            .values
            .insert("unknown.key".into(), SettingValue::text("x"));
        let model = SettingsModel::new(SettingsContext::new());
        assert!(model.attach(SettingsScope::User, repository).is_err());
    }

    #[test]
    fn extension_save_failure_does_not_publish_or_change_state() {
        let model = SettingsModel::new(SettingsContext::new());
        model
            .attach(
                SettingsScope::User,
                Arc::new(MemoryRepository::failing(SettingsScope::User)),
            )
            .unwrap();
        model
            .register_extension_settings("dev.example.terminal", vec![extension_definition()])
            .unwrap();
        let watch = model.subscribe();
        let key = "extensions/dev.example.terminal/font-size";
        assert!(
            model
                .update_extension(
                    SettingsScope::User,
                    &[(key, Some(SettingValue::integer(18)))]
                )
                .is_err()
        );
        assert!(watch.try_recv().is_err());
        assert_eq!(
            model.resolved().extension_setting(key).unwrap().value(),
            Some(&SettingValue::integer(12))
        );
    }

    #[test]
    fn extension_scopes_override_without_leaking_other_projects() {
        let context = SettingsContext::new().with_project(ProjectId::new("lapis"));
        let model = SettingsModel::new(context);
        let key = "extensions/dev.example.terminal/font-size";
        let mut definition = extension_definition();
        definition.scopes.push(settings::ScopeKind::ProjectLocal);
        model
            .register_extension_settings("dev.example.terminal", vec![definition])
            .unwrap();
        let user = Arc::new(MemoryRepository::new(SettingsScope::User));
        let local_scope = SettingsScope::ProjectLocal {
            id: ProjectId::new("lapis"),
        };
        let local = Arc::new(MemoryRepository::new(local_scope.clone()));
        model.attach(SettingsScope::User, user).unwrap();
        model.attach(local_scope.clone(), local).unwrap();
        model
            .update_extension(
                SettingsScope::User,
                &[(key, Some(SettingValue::integer(13)))],
            )
            .unwrap();
        model
            .update_extension(
                local_scope.clone(),
                &[(key, Some(SettingValue::integer(17)))],
            )
            .unwrap();
        let resolved = model.resolved();
        let setting = resolved.extension_setting(key).unwrap();
        assert_eq!(setting.value(), Some(&SettingValue::integer(17)));
        assert_eq!(setting.origin(), Some(&local_scope));
        let forbidden = SettingsScope::Host {
            id: HostId::new("host"),
        };
        assert!(
            model
                .update_extension(forbidden, &[(key, Some(SettingValue::integer(22)))])
                .is_err()
        );
        model.update_extension(local_scope, &[(key, None)]).unwrap();
        assert_eq!(
            model.resolved().extension_setting(key).unwrap().value(),
            Some(&SettingValue::integer(13))
        );
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

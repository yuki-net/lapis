//! 接続単位の拡張宣言。Host の変更を共有設定の正本にしない。
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, Weak, mpsc},
};

use client_api::{HostExtension, HostExtensionContributions, HostExtensionsListResponse};
use localization::{ExtensionLocalizations, LocaleId, LocalePack, MessageId};
use settings::{
    ExtensionSettingsCatalog, ResolvedSettings, SettingValue, SettingsError, SettingsScope,
};

use crate::SettingsModel;

const MAX_DECLARATIONS: usize = 256;
const MAX_BYTES: usize = 256 * 1024;

/// 特定の接続で有効な宣言。値の保存先は持たない。
#[derive(Clone)]
pub struct ExtensionContributionSnapshot(Arc<Snapshot>);

struct Snapshot {
    extensions: Vec<HostExtension>,
    contributions: Vec<HostExtensionContributions>,
    settings: ExtensionSettingsCatalog,
    translations: ExtensionLocalizations,
    core_locales: Vec<LocalePack>,
}

impl ExtensionContributionSnapshot {
    pub fn extensions(&self) -> &[HostExtension] {
        &self.0.extensions
    }
    pub fn contributions(&self) -> &[HostExtensionContributions] {
        &self.0.contributions
    }
    /// 現在稼働中の拡張から表示宣言を得る。enabled は次回起動用。
    /// 現在稼働中の拡張が宣言した View を、提供元と共に列挙する。
    /// enabled は次回起動時の設定なので、現在の表示可否には active を使う。
    pub fn active_views(
        &self,
    ) -> impl Iterator<Item = (&str, &extension_ui_contract::ContributedView)> + '_ {
        self.contributions()
            .iter()
            .filter(move |entry| {
                self.extensions().iter().any(|extension| {
                    extension.id == entry.extension_id && extension.active == Some(true)
                })
            })
            .flat_map(|entry| {
                entry.views.iter().flat_map(move |views| {
                    views
                        .views
                        .iter()
                        .map(move |view| (entry.extension_id.as_str(), view))
                })
            })
    }

    pub fn active_view(
        &self,
        owner: &str,
        content_type: &str,
    ) -> Option<&extension_ui_contract::ContributedView> {
        self.active_views()
            .find(|(extension, view)| *extension == owner && view.content_type == content_type)
            .map(|(_, view)| view)
    }

    pub fn has_active_view(&self, owner: &str, content_type: &str) -> bool {
        self.active_view(owner, content_type).is_some()
    }
    /// 拡張が提供する Host 標準 UI の翻訳。
    pub fn core_locales(&self) -> &[LocalePack] {
        &self.0.core_locales
    }

    pub fn resolve_text(&self, owner: &str, locale: &LocaleId, message: &MessageId) -> String {
        self.0.translations.resolve(owner, locale, message)
    }
}

struct State {
    source: u64,
    model: Weak<SettingsModel>,
    generation: u64,
    current: Option<ExtensionContributionSnapshot>,
    subscribers: Vec<mpsc::Sender<Option<ExtensionContributionSnapshot>>>,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(model) = self.model.upgrade() {
            model.remove_contribution_source(self.source);
        }
    }
}

/// 1 接続が所有する宣言と非同期応答の世代。複数接続で共有しない。
#[derive(Clone)]
pub struct ExtensionContributionSession {
    model: Arc<SettingsModel>,
    state: Arc<Mutex<State>>,
}

impl ExtensionContributionSession {
    pub(crate) fn new(model: Arc<SettingsModel>) -> Self {
        let source = model.new_contribution_source();
        let state = State {
            source,
            model: Arc::downgrade(&model),
            generation: 0,
            current: None,
            subscribers: Vec::new(),
        };
        Self {
            model,
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// 応答を要求する前に呼ぶ。同じ接続の古い応答を無効にする。
    pub fn begin_reload(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.generation = state
            .generation
            .checked_add(1)
            .expect("extension generation overflow");
        state.generation
    }

    /// 接続終了時はこの接続の宣言だけを除き、保存済み設定は残す。
    pub fn disconnect(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation = state
            .generation
            .checked_add(1)
            .expect("extension generation overflow");
        self.model.remove_contribution_source(state.source);
        state.current = None;
        Self::publish(&mut state);
    }

    /// 世代を維持したまま切断中の古い値を隠し、同じ要求の再試行を許す。
    pub fn clear_if_current(&self, generation: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.generation != generation {
            return false;
        }
        if state.current.is_some() {
            self.model.remove_contribution_source(state.source);
            state.current = None;
            Self::publish(&mut state);
        }
        true
    }

    pub fn snapshot(&self) -> Option<ExtensionContributionSnapshot> {
        self.state.lock().unwrap().current.clone()
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Option<ExtensionContributionSnapshot>> {
        let (sender, receiver) = mpsc::channel();
        self.state.lock().unwrap().subscribers.push(sender);
        receiver
    }

    /// 全宣言と既存の保存値を検証してから一括適用する。古い応答は false。
    pub fn apply(
        &self,
        generation: u64,
        response: HostExtensionsListResponse,
    ) -> Result<bool, SettingsError> {
        if self.state.lock().unwrap().generation != generation {
            return Ok(false);
        }
        let snapshot = build_snapshot(response.extensions, response.contributions);
        let mut state = self.state.lock().unwrap();
        if state.generation != generation {
            return Ok(false);
        }
        let snapshot = snapshot?;
        let replacement = snapshot.clone();
        self.model
            .with_validated_extension_catalog(state.source, &snapshot.0.settings, || {
                state.current = Some(replacement);
                Self::publish(&mut state);
            })?;
        Ok(true)
    }

    pub fn resolved(&self) -> Option<ResolvedSettings> {
        let state = self.state.lock().unwrap();
        state.current.as_ref().map(|snapshot| {
            self.model
                .resolved_with_extension_catalog(&snapshot.0.settings)
        })
    }

    /// 接続が保持する定義と許可されたスコープで保存する。I/O を含むため worker で呼ぶ。
    pub fn update(
        &self,
        scope: SettingsScope,
        values: &[(&str, Option<SettingValue>)],
    ) -> Result<(), SettingsError> {
        let mut state = self.state.lock().unwrap();
        let snapshot = state
            .current
            .as_ref()
            .ok_or_else(|| SettingsError::new("拡張の宣言がありません"))?;
        self.model
            .update_with_extension_catalog(&snapshot.0.settings, scope, values)?;
        Self::publish(&mut state);
        Ok(())
    }

    fn publish(state: &mut State) {
        let snapshot = state.current.clone();
        state
            .subscribers
            .retain(|sender| sender.send(snapshot.clone()).is_ok());
    }
}

fn locale_pack(
    locale: &str,
    messages: &std::collections::BTreeMap<String, String>,
) -> Result<LocalePack, SettingsError> {
    let manifest =
        serde_json::json!({"id": locale, "name": locale, "version": "1", "fallback": "en-US"});
    let contents =
        serde_json::to_string(messages).map_err(|error| SettingsError::new(error.to_string()))?;
    LocalePack::from_json(&manifest.to_string(), &[&contents])
        .map_err(|error| SettingsError::new(format!("翻訳データが不正です: {error:?}")))
}

fn build_snapshot(
    extensions: Vec<HostExtension>,
    contributions: Vec<HostExtensionContributions>,
) -> Result<ExtensionContributionSnapshot, SettingsError> {
    if extensions.len() > MAX_DECLARATIONS
        || contributions.len() > MAX_DECLARATIONS
        || serde_json::to_vec(&extensions)
            .map_err(|error| SettingsError::new(error.to_string()))?
            .len()
            > MAX_BYTES
        || serde_json::to_vec(&contributions)
            .map_err(|error| SettingsError::new(error.to_string()))?
            .len()
            > MAX_BYTES
    {
        return Err(SettingsError::new("拡張宣言が大きすぎます"));
    }
    let mut extension_ids = BTreeSet::new();
    if extensions.iter().any(|entry| {
        !valid_extension_id(&entry.id)
            || !extension_ids.insert(entry.id.clone())
            || entry.version.len() > 128
    }) {
        return Err(SettingsError::new("拡張目録が不正です"));
    }
    let mut settings = ExtensionSettingsCatalog::default();
    let mut translations = ExtensionLocalizations::default();
    let mut core_locales = Vec::new();
    let mut owners = BTreeSet::new();
    for contribution in &contributions {
        let owner = &contribution.extension_id;
        if !valid_extension_id(owner) || owner == "lapis.core" {
            return Err(SettingsError::new(format!("拡張 ID が不正です: {owner}")));
        }
        if !owners.insert(owner.clone()) {
            return Err(SettingsError::new(format!(
                "拡張 ID が重複しています: {owner}"
            )));
        }
        let declaration = &contribution.declaration;
        if declaration
            .translations
            .iter()
            .any(|entry| !valid_extension_id(&entry.target))
        {
            return Err(SettingsError::new("翻訳先の拡張 ID が不正です"));
        }
        declaration.validate(owner).map_err(SettingsError::new)?;
        if let Some(views) = &contribution.views {
            views
                .validate(owner, declaration)
                .map_err(SettingsError::new)?;
        }
        settings.register(
            owner,
            declaration
                .settings
                .iter()
                .map(|setting| setting.definition.clone())
                .collect(),
        )?;
        let mut packs = Vec::new();
        for translation in declaration
            .translations
            .iter()
            .filter(|entry| entry.target == *owner)
        {
            packs.push(locale_pack(&translation.locale, &translation.messages)?);
        }
        if !declaration
            .translations
            .iter()
            .any(|entry| entry.target == *owner && entry.locale == declaration.default_locale)
        {
            packs.push(locale_pack(
                &declaration.default_locale,
                &Default::default(),
            )?);
        }
        translations
            .register(
                owner.clone(),
                Some(LocaleId::new(&declaration.default_locale)),
                packs,
            )
            .map_err(|error| SettingsError::new(format!("拡張の翻訳が不正です: {error:?}")))?;
    }
    for contribution in &contributions {
        let mut packs = Vec::new();
        for translation in contribution
            .declaration
            .translations
            .iter()
            .filter(|entry| entry.target != contribution.extension_id)
        {
            if translation.target == "lapis.core" {
                if translation.locale.eq_ignore_ascii_case("en-US") {
                    return Err(SettingsError::new("Host の既定言語は置換できません"));
                }
                core_locales.push(locale_pack(&translation.locale, &translation.messages)?);
            }
            packs.push((
                translation.target.clone(),
                locale_pack(&translation.locale, &translation.messages)?,
            ));
        }
        translations
            .register_translations(contribution.extension_id.clone(), packs)
            .map_err(|error| {
                SettingsError::new(format!("翻訳の提供元が重複しています: {error:?}"))
            })?;
    }
    Ok(ExtensionContributionSnapshot(Arc::new(Snapshot {
        extensions,
        contributions,
        settings,
        translations,
        core_locales,
    })))
}

fn valid_extension_id(id: &str) -> bool {
    let stem = id.split('.').next().unwrap_or_default();
    let device_number = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
    };
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
        && !id.ends_with('.')
        && !id.starts_with(".staging-")
        && !["catalog.json", "data", "installed"].contains(&id)
        && !["con", "prn", "aux", "nul"].contains(&stem)
        && !device_number("com")
        && !device_number("lpt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SettingsSession;
    use serde_json::json;
    use settings::{SettingsDocument, SettingsRepository};

    struct Memory(Mutex<SettingsDocument>);
    impl SettingsRepository for Memory {
        fn load(&self) -> Result<SettingsDocument, SettingsError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, document: &SettingsDocument) -> Result<(), SettingsError> {
            *self.0.lock().unwrap() = document.clone();
            Ok(())
        }
    }

    fn response(default: i64) -> HostExtensionsListResponse {
        serde_json::from_value(json!({"extensions":[], "contributions":[{
            "extension_id":"dev.example.terminal",
            "declaration":{"schema":1,"default_locale":"en-US",
                "settings":[{"definition":{"name":"size","value_kind":"integer","scopes":["user"],"default":default,"allowed_values":null},"label":"terminal.size"}],
                "translations":[{"target":"dev.example.terminal","locale":"en-US","messages":{"terminal.size":"Size"}}]}
        },{
            "extension_id":"dev.example.japanese",
            "declaration":{"schema":1,"translations":[{"target":"dev.example.terminal","locale":"ja-JP","messages":{"terminal.size":"サイズ"}}]}
        }]})).unwrap()
    }

    #[test]
    fn view_availability_uses_running_state_and_matching_declaration() {
        let mut response: HostExtensionsListResponse = serde_json::from_value(json!({
            "extensions":[{"id":"dev.example.terminal", "version":"1.0.0", "installed":true, "enabled":false, "active":true}],
            "contributions":[{"extension_id":"dev.example.terminal",
                "declaration":{"schema":1,"translations":[{"target":"dev.example.terminal","locale":"en-US","messages":{"terminal.title":"Terminal"}},{"target":"dev.example.terminal","locale":"ja-JP","messages":{"terminal.title":"ターミナル"}}]},
                "views":{"schema":1,"views":[{"id":"dev.example.terminal.view", "content_type":"example.grid.v1", "title":"terminal.title", "default_zone":"bottom"}]}}]
        })).unwrap();
        let check = |response: &HostExtensionsListResponse| {
            build_snapshot(response.extensions.clone(), response.contributions.clone())
                .unwrap()
                .has_active_view("dev.example.terminal", "example.grid.v1")
        };
        assert!(check(&response)); // enabled は次回起動の設定。
        let snapshot =
            build_snapshot(response.extensions.clone(), response.contributions.clone()).unwrap();
        let view = snapshot
            .active_view("dev.example.terminal", "example.grid.v1")
            .unwrap();
        assert_eq!(view.id, "dev.example.terminal.view");
        assert_eq!(
            view.default_zone,
            extension_ui_contract::ContributedZone::Bottom
        );
        assert_eq!(
            snapshot.resolve_text(
                "dev.example.terminal",
                &LocaleId::new("fr-FR"),
                &MessageId::new(view.title.clone())
            ),
            "Terminal"
        );
        assert_eq!(
            snapshot.resolve_text(
                "dev.example.terminal",
                &LocaleId::new("ja-JP"),
                &MessageId::new(view.title.clone())
            ),
            "ターミナル"
        );
        response.extensions[0].active = Some(false);
        assert!(!check(&response));
        response.extensions[0].active = None;
        assert!(!check(&response));
        response.extensions[0].active = Some(true);
        response.contributions[0].views.as_mut().unwrap().views[0].content_type =
            "other.grid".into();
        assert!(!check(&response));
    }

    #[test]
    fn active_views_include_all_running_owners_and_keep_declared_identity() {
        let response: HostExtensionsListResponse = serde_json::from_value(json!({
            "extensions": [
                {"id":"example.one", "version":"1.0.0", "installed":true, "enabled":false, "active":true},
                {"id":"example.two", "version":"1.0.0", "installed":true, "enabled":true, "active":true},
                {"id":"example.disabled", "version":"1.0.0", "installed":true, "enabled":true, "active":false}
            ],
            "contributions": [
                {"extension_id":"example.one", "declaration":{"schema":1,"translations":[{"target":"example.one","locale":"en-US","messages":{"title":"One"}}]},
                 "views":{"schema":1,"views":[{"id":"example.one.view","content_type":"example.grid.v1","title":"title","default_zone":"left"}]}},
                {"extension_id":"example.two", "declaration":{"schema":1,"translations":[{"target":"example.two","locale":"en-US","messages":{"title":"Two"}}]},
                 "views":{"schema":1,"views":[{"id":"example.two.view","content_type":"example.grid.v1","title":"title","default_zone":"bottom"}]}},
                {"extension_id":"example.disabled", "declaration":{"schema":1,"translations":[{"target":"example.disabled","locale":"en-US","messages":{"title":"Disabled"}}]},
                 "views":{"schema":1,"views":[{"id":"example.disabled.view","content_type":"example.grid.v1","title":"title","default_zone":"right"}]}}
            ]
        })).unwrap();
        let snapshot = build_snapshot(response.extensions, response.contributions).unwrap();
        let views: Vec<_> = snapshot
            .active_views()
            .map(|(owner, view)| (owner, view.id.as_str()))
            .collect();
        assert_eq!(
            views,
            [
                ("example.one", "example.one.view"),
                ("example.two", "example.two.view")
            ]
        );
    }

    fn session() -> (SettingsSession, Arc<Memory>) {
        let repository = Arc::new(Memory(Mutex::new(SettingsDocument::empty(
            SettingsScope::User,
        ))));
        (
            SettingsSession::load(repository.clone()).unwrap(),
            repository,
        )
    }

    #[test]
    fn core_translation_reaches_localizer_and_falls_back_after_disconnect() {
        let (settings, _) = session();
        let session = settings.extension_contributions();
        let generation = session.begin_reload();
        let response = serde_json::from_value(json!({"extensions":[], "contributions":[{
            "extension_id":"dev.example.french",
            "declaration":{"schema":1,"translations":[{"target":"lapis.core","locale":"fr-FR","messages":{"command.save-document":"Enregistrer"}}]}
        }]})).unwrap();
        assert!(session.apply(generation, response).unwrap());
        let mut localizer = localization::Localizer::bundled();
        localizer
            .replace_external(session.snapshot().unwrap().core_locales())
            .unwrap();
        assert!(localizer.set_active(&LocaleId::new("fr-FR")));
        assert_eq!(
            localizer.resolve(&MessageId::new("command.save-document")),
            "Enregistrer"
        );
        session.disconnect();
        localizer.replace_external(&[]).unwrap();
        assert_eq!(localizer.active(), &LocaleId::new("en-US"));
    }

    #[test]
    fn accepts_only_latest_reply_and_clears_only_its_own_connection() {
        let (settings, _) = session();
        let first = settings.extension_contributions();
        let second = settings.extension_contributions();
        let events = first.subscribe();
        let stale = first.begin_reload();
        let current = first.begin_reload();
        let second_generation = second.begin_reload();
        assert!(first.apply(current, response(12)).unwrap());
        assert!(events.try_recv().unwrap().is_some());
        assert!(second.apply(second_generation, response(12)).unwrap());
        assert!(!first.apply(stale, response(99)).unwrap());
        let snapshot = first.snapshot().unwrap();
        assert_eq!(
            snapshot.resolve_text(
                "dev.example.terminal",
                &LocaleId::new("ja-JP"),
                &MessageId::new("terminal.size")
            ),
            "サイズ"
        );
        assert_eq!(
            snapshot.resolve_text(
                "dev.example.terminal",
                &LocaleId::new("fr-FR"),
                &MessageId::new("terminal.size")
            ),
            "Size"
        );
        first.disconnect();
        assert!(events.try_recv().unwrap().is_none());
        assert!(first.snapshot().is_none());
        assert!(second.snapshot().is_some());
        assert!(!first.apply(current, response(12)).unwrap());
    }

    #[test]
    fn invalid_or_conflicting_declarations_never_replace_the_snapshot() {
        let (settings, _) = session();
        let first = settings.extension_contributions();
        let second = settings.extension_contributions();
        let first_generation = first.begin_reload();
        first.apply(first_generation, response(12)).unwrap();
        let second_generation = second.begin_reload();
        assert!(second.apply(second_generation, response(20)).is_err());
        assert!(second.snapshot().is_none());
        assert_eq!(
            first
                .resolved()
                .unwrap()
                .extension_setting("extensions/dev.example.terminal/size")
                .unwrap()
                .value(),
            Some(&SettingValue::integer(12))
        );
        let mut duplicate = response(12);
        duplicate
            .contributions
            .push(duplicate.contributions[0].clone());
        let generation = first.begin_reload();
        assert!(first.apply(generation, duplicate).is_err());
        assert_eq!(first.snapshot().unwrap().contributions().len(), 2);
        let mut invalid_id = response(12);
        invalid_id.contributions[0].extension_id = "CON".to_owned();
        assert!(first.apply(generation, invalid_id).is_err());
        assert_eq!(first.snapshot().unwrap().contributions().len(), 2);
        let mut invalid_view = response(12);
        invalid_view.contributions[0].views = Some(
            serde_json::from_value(json!({
                "schema": 1,
                "views": [{
                    "id": "another.extension.view",
                    "content_type": "lapis.content.terminal-grid.v1",
                    "title": "terminal.size",
                    "default_zone": "bottom",
                    "multiple_instances": true,
                    "open_command": "dev.example.terminal.open"
                }]
            }))
            .unwrap(),
        );
        assert!(first.apply(generation, invalid_view).is_err());
        assert_eq!(first.snapshot().unwrap().contributions().len(), 2);
        first.disconnect();
        assert!(second.apply(second_generation, response(20)).unwrap());
    }

    #[test]
    fn scope_validation_and_persistent_values_survive_disconnect() {
        let (settings, repository) = session();
        let source = settings.extension_contributions();
        let generation = source.begin_reload();
        source.apply(generation, response(12)).unwrap();
        let key = "extensions/dev.example.terminal/size";
        assert!(
            source
                .update(
                    SettingsScope::User,
                    &[(key, Some(SettingValue::text("bad")))]
                )
                .is_err()
        );
        source
            .update(
                SettingsScope::User,
                &[(key, Some(SettingValue::integer(18)))],
            )
            .unwrap();
        assert_eq!(
            repository.0.lock().unwrap().values.get(key),
            Some(&SettingValue::integer(18))
        );
        source.disconnect();
        assert!(
            source
                .update(
                    SettingsScope::User,
                    &[(key, Some(SettingValue::integer(19)))]
                )
                .is_err()
        );
        let generation = source.begin_reload();
        source.apply(generation, response(12)).unwrap();
        assert_eq!(
            source
                .resolved()
                .unwrap()
                .extension_setting(key)
                .unwrap()
                .value(),
            Some(&SettingValue::integer(18))
        );
        assert!(
            source
                .update(
                    SettingsScope::Host {
                        id: settings::HostId::new("other")
                    },
                    &[(key, Some(SettingValue::integer(19)))]
                )
                .is_err()
        );
    }
}

use std::collections::BTreeMap;

use crate::{LocaleId, LocalePack, LocalePackError, MessageId, bundled};

/// 選択中ロケールと英語フォールバックから文言を解決する。
pub struct Localizer {
    fallback: LocaleId,
    active: LocaleId,
    packs: BTreeMap<LocaleId, LocalePack>,
    external: BTreeMap<LocaleId, LocalePack>,
}

impl Localizer {
    pub fn bundled() -> Self {
        let mut localizer = Self::new(LocaleId::new("en-US"));
        for pack in bundled::packs() {
            localizer
                .register(pack)
                .expect("bundled locales must be unique");
        }
        localizer
    }

    pub fn new(fallback: LocaleId) -> Self {
        Self {
            active: fallback.clone(),
            fallback,
            packs: BTreeMap::new(),
            external: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, pack: LocalePack) -> Result<(), LocalePackError> {
        let id = pack.manifest.id.clone();
        if self.packs.contains_key(&id) {
            return Err(LocalePackError::DuplicateLocale(id));
        }
        self.packs.insert(id, pack);
        Ok(())
    }

    pub fn set_active(&mut self, locale: &LocaleId) -> bool {
        if !self.packs.contains_key(locale) && !self.external.contains_key(locale) {
            return false;
        }
        self.active = locale.clone();
        true
    }

    /// 接続先が提供する言語パックを原子的に置き換える。
    pub fn replace_external(&mut self, packs: &[LocalePack]) -> Result<(), LocalePackError> {
        let mut replacement = BTreeMap::new();
        for pack in packs {
            let id = pack.manifest.id.clone();
            if id == self.fallback || replacement.insert(id.clone(), pack.clone()).is_some() {
                return Err(LocalePackError::DuplicateLocale(id));
            }
        }
        self.external = replacement;
        if !self.packs.contains_key(&self.active) && !self.external.contains_key(&self.active) {
            self.active = self.fallback.clone();
        }
        Ok(())
    }

    pub fn available(&self) -> Vec<(LocaleId, String)> {
        let mut locales: BTreeMap<_, _> = self
            .packs
            .iter()
            .map(|(id, pack)| (id.clone(), pack.manifest.name.clone()))
            .collect();
        locales.extend(
            self.external
                .iter()
                .map(|(id, pack)| (id.clone(), pack.manifest.name.clone())),
        );
        locales.into_iter().collect()
    }

    /// 現在選択中のロケールを返す。
    pub fn active(&self) -> &LocaleId {
        &self.active
    }

    pub fn resolve(&self, message: &MessageId) -> String {
        self.external
            .get(&self.active)
            .and_then(|pack| pack.resolve(message))
            .or_else(|| {
                self.packs
                    .get(&self.active)
                    .and_then(|pack| pack.resolve(message))
            })
            .or_else(|| {
                self.packs
                    .get(&self.fallback)
                    .and_then(|pack| pack.resolve(message))
            })
            .unwrap_or_else(|| message.as_str())
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use crate::{LocaleId, Localizer, MessageId};

    #[test]
    fn bundled_locale_falls_back_per_message() {
        let mut localizer = Localizer::bundled();
        assert!(localizer.set_active(&LocaleId::new("ja-JP")));
        assert_eq!(
            localizer.resolve(&MessageId::new("command.save-document")),
            "保存"
        );
        assert_eq!(
            localizer.resolve(&MessageId::new("extension.unknown")),
            "extension.unknown"
        );
    }

    #[test]
    fn external_language_is_selectable_and_removal_restores_english() {
        let mut localizer = Localizer::bundled();
        let pack = crate::LocalePack::from_json(
            r#"{"id":"fr-FR","name":"Français","version":"1","fallback":"en-US"}"#,
            &[r#"{"command.save-document":"Enregistrer"}"#],
        )
        .unwrap();
        localizer.replace_external(&[pack]).unwrap();
        assert!(localizer.set_active(&LocaleId::new("fr-FR")));
        assert_eq!(
            localizer.resolve(&MessageId::new("command.save-document")),
            "Enregistrer"
        );
        assert_eq!(
            localizer.resolve(&MessageId::new("settings.page.appearance.theme")),
            Localizer::bundled().resolve(&MessageId::new("settings.page.appearance.theme"))
        );
        localizer.replace_external(&[]).unwrap();
        assert_eq!(localizer.active(), &LocaleId::new("en-US"));
    }
}

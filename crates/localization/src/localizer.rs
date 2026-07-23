use std::collections::BTreeMap;

use crate::{LocaleId, LocalePack, LocalePackError, MessageId, bundled};

/// 選択中ロケールと英語フォールバックから文言を解決する。
pub struct Localizer {
    fallback: LocaleId,
    active: LocaleId,
    packs: BTreeMap<LocaleId, LocalePack>,
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
        if !self.packs.contains_key(locale) {
            return false;
        }
        self.active = locale.clone();
        true
    }

    pub fn resolve(&self, message: &MessageId) -> String {
        self.packs
            .get(&self.active)
            .and_then(|pack| pack.resolve(message))
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
}

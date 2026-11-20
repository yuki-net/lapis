use std::collections::{BTreeMap, btree_map::Entry};

use crate::{LocaleId, LocalePack, MessageId};

/// 拡張ごとの表示言語を所有する。各拡張は自分の文言だけを解決する。
#[derive(Default)]
pub struct ExtensionLocalizations {
    extensions: BTreeMap<String, ExtensionPacks>,
    translations: BTreeMap<(String, LocaleId), TranslationPack>,
}

struct TranslationPack {
    provider: String,
    pack: LocalePack,
}

struct ExtensionPacks {
    fallback: LocaleId,
    packs: BTreeMap<LocaleId, LocalePack>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionLocalizationError {
    EmptyExtensionId,
    DuplicateExtension(String),
    DuplicateLocale(LocaleId),
    MissingFallback(LocaleId),
    DuplicateTranslation {
        extension_id: String,
        locale: LocaleId,
    },
}

impl ExtensionLocalizations {
    /// 拡張の文言をまとめて登録する。既定言語を省略した場合は en-US を使う。
    /// 登録に失敗した場合は既存の登録を変更しない。
    pub fn register(
        &mut self,
        extension_id: impl Into<String>,
        fallback: Option<LocaleId>,
        packs: impl IntoIterator<Item = LocalePack>,
    ) -> Result<(), ExtensionLocalizationError> {
        let extension_id = extension_id.into();
        if extension_id.is_empty() {
            return Err(ExtensionLocalizationError::EmptyExtensionId);
        }
        if self.extensions.contains_key(&extension_id) {
            return Err(ExtensionLocalizationError::DuplicateExtension(extension_id));
        }
        let fallback = fallback.unwrap_or_else(|| LocaleId::new("en-US"));
        let mut locales = BTreeMap::new();
        for pack in packs {
            let id = pack.manifest.id.clone();
            match locales.entry(id.clone()) {
                Entry::Occupied(_) => return Err(ExtensionLocalizationError::DuplicateLocale(id)),
                Entry::Vacant(entry) => {
                    entry.insert(pack);
                }
            }
        }
        if !locales.contains_key(&fallback) {
            return Err(ExtensionLocalizationError::MissingFallback(fallback));
        }
        self.extensions.insert(
            extension_id,
            ExtensionPacks {
                fallback,
                packs: locales,
            },
        );
        Ok(())
    }

    /// 拡張を解除する。別の拡張の文言には影響しない。
    pub fn unregister(&mut self, extension_id: &str) -> bool {
        self.extensions.remove(extension_id).is_some()
    }

    /// 言語パックが他の拡張に提供する翻訳をまとめて登録する。
    /// 同じ拡張と言語の提供元が重複する場合は、既存の登録を変えない。
    pub fn register_translations(
        &mut self,
        provider: impl Into<String>,
        packs: impl IntoIterator<Item = (String, LocalePack)>,
    ) -> Result<(), ExtensionLocalizationError> {
        let provider = provider.into();
        if provider.is_empty() {
            return Err(ExtensionLocalizationError::EmptyExtensionId);
        }
        let mut pending = BTreeMap::new();
        for (target, pack) in packs {
            if target.is_empty() {
                return Err(ExtensionLocalizationError::EmptyExtensionId);
            }
            let locale = pack.manifest.id.clone();
            let key = (target.clone(), locale.clone());
            if self.translations.contains_key(&key) || pending.contains_key(&key) {
                return Err(ExtensionLocalizationError::DuplicateTranslation {
                    extension_id: target,
                    locale,
                });
            }
            pending.insert(
                key,
                TranslationPack {
                    provider: provider.clone(),
                    pack,
                },
            );
        }
        self.translations.extend(pending);
        Ok(())
    }

    /// 言語パックの無効化時に、その提供元の翻訳だけを外す。
    pub fn unregister_translations(&mut self, provider: &str) {
        self.translations
            .retain(|_, translation| translation.provider != provider);
    }

    /// 選択言語、拡張の既定言語の順に探し、無い場合はメッセージ ID を返す。
    pub fn resolve(&self, extension_id: &str, active: &LocaleId, message: &MessageId) -> String {
        self.extensions
            .get(extension_id)
            .and_then(|extension| {
                extension
                    .packs
                    .get(active)
                    .and_then(|pack| pack.resolve(message))
                    .or_else(|| {
                        self.translations
                            .get(&(extension_id.to_owned(), active.clone()))
                            .and_then(|translation| translation.pack.resolve(message))
                    })
                    .or_else(|| {
                        extension
                            .packs
                            .get(&extension.fallback)
                            .and_then(|pack| pack.resolve(message))
                            .or_else(|| {
                                self.translations
                                    .get(&(extension_id.to_owned(), extension.fallback.clone()))
                                    .and_then(|translation| translation.pack.resolve(message))
                            })
                    })
            })
            .unwrap_or_else(|| message.as_str())
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(locale: &str, entries: &[(&str, &str)]) -> LocalePack {
        let manifest = serde_json::json!({
            "id": locale, "name": locale, "version": "1", "fallback": "en-US"
        });
        let document: BTreeMap<_, _> = entries.iter().copied().collect();
        LocalePack::from_json(
            &manifest.to_string(),
            &[&serde_json::to_string(&document).unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn extensions_isolate_messages_and_fallback_per_extension() {
        let mut translations = ExtensionLocalizations::default();
        translations
            .register(
                "terminal",
                None,
                [
                    pack("en-US", &[("title", "Terminal"), ("close", "Close")]),
                    pack("ja-JP", &[("title", "端末")]),
                ],
            )
            .unwrap();
        translations
            .register(
                "other",
                Some(LocaleId::new("fr-FR")),
                [pack("fr-FR", &[("title", "Autre")])],
            )
            .unwrap();
        let ja = LocaleId::new("ja-JP");
        assert_eq!(
            translations.resolve("terminal", &ja, &MessageId::new("title")),
            "端末"
        );
        assert_eq!(
            translations.resolve("terminal", &ja, &MessageId::new("close")),
            "Close"
        );
        assert_eq!(
            translations.resolve("other", &ja, &MessageId::new("title")),
            "Autre"
        );
        assert_eq!(
            translations.resolve("other", &ja, &MessageId::new("close")),
            "close"
        );
        assert_eq!(
            translations.resolve("missing", &ja, &MessageId::new("title")),
            "title"
        );
        assert!(translations.unregister("terminal"));
        assert_eq!(
            translations.resolve("terminal", &ja, &MessageId::new("title")),
            "title"
        );
        assert_eq!(
            translations.resolve("other", &ja, &MessageId::new("title")),
            "Autre"
        );
    }

    #[test]
    fn invalid_registration_does_not_change_existing_extensions() {
        let mut translations = ExtensionLocalizations::default();
        assert_eq!(
            translations.register("terminal", None, [pack("ja-JP", &[])]),
            Err(ExtensionLocalizationError::MissingFallback(LocaleId::new(
                "en-US"
            )))
        );
        assert_eq!(
            translations.register("terminal", None, [pack("en-US", &[]), pack("en-US", &[])]),
            Err(ExtensionLocalizationError::DuplicateLocale(LocaleId::new(
                "en-US"
            )))
        );
        translations
            .register("terminal", None, [pack("en-US", &[("title", "Terminal")])])
            .unwrap();
        assert!(matches!(
            translations.register("terminal", None, [pack("en-US", &[("title", "Other")])]),
            Err(ExtensionLocalizationError::DuplicateExtension(_))
        ));
        assert_eq!(
            translations.resolve(
                "terminal",
                &LocaleId::new("en-US"),
                &MessageId::new("title")
            ),
            "Terminal"
        );
    }

    #[test]
    fn language_pack_contributes_and_removes_only_its_own_translations() {
        let mut translations = ExtensionLocalizations::default();
        translations
            .register(
                "terminal",
                None,
                [pack("en-US", &[("title", "Terminal"), ("close", "Close")])],
            )
            .unwrap();
        translations
            .register("other", None, [pack("en-US", &[("title", "Other")])])
            .unwrap();
        translations
            .register_translations(
                "japanese",
                [
                    ("terminal".to_owned(), pack("ja-JP", &[("title", "端末")])),
                    ("other".to_owned(), pack("ja-JP", &[("title", "その他")])),
                ],
            )
            .unwrap();
        let ja = LocaleId::new("ja-JP");
        assert_eq!(
            translations.resolve("terminal", &ja, &MessageId::new("title")),
            "端末"
        );
        assert_eq!(
            translations.resolve("terminal", &ja, &MessageId::new("close")),
            "Close"
        );
        assert!(matches!(
            translations.register_translations(
                "duplicate",
                [("terminal".to_owned(), pack("ja-JP", &[("title", "重複")]))]
            ),
            Err(ExtensionLocalizationError::DuplicateTranslation { .. })
        ));
        translations
            .register_translations(
                "french",
                [(
                    "terminal".to_owned(),
                    pack("fr-FR", &[("title", "Terminal FR")]),
                )],
            )
            .unwrap();
        translations.unregister_translations("japanese");
        assert_eq!(
            translations.resolve("terminal", &ja, &MessageId::new("title")),
            "Terminal"
        );
        assert_eq!(
            translations.resolve("other", &ja, &MessageId::new("title")),
            "Other"
        );
        assert_eq!(
            translations.resolve(
                "terminal",
                &LocaleId::new("fr-FR"),
                &MessageId::new("title")
            ),
            "Terminal FR"
        );
    }
}

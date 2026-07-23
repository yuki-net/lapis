use crate::LocalePack;

pub(crate) fn packs() -> [LocalePack; 2] {
    [
        LocalePack::from_json(
            include_str!("../../../locales/en-US/manifest.json"),
            &[include_str!("../../../locales/en-US/messages/common.json")],
        ),
        LocalePack::from_json(
            include_str!("../../../locales/ja-JP/manifest.json"),
            &[include_str!("../../../locales/ja-JP/messages/common.json")],
        ),
    ]
    .map(|pack| pack.expect("bundled locale pack must be valid"))
}

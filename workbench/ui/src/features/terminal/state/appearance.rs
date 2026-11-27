use desktop_client::ResolvedSettings;

fn extension_setting<'a>(
    values: &'a ResolvedSettings,
    name: &str,
) -> Option<&'a settings::SettingValue> {
    values
        .extension_setting(&format!("extensions/lapis.terminal/{name}"))
        .and_then(|setting| setting.value())
}

pub(super) fn terminal_font_family(values: &ResolvedSettings) -> Option<String> {
    extension_setting(values, "font-family")
        .and_then(settings::SettingValue::as_text)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| values.terminal_font_family())
}

pub(super) fn terminal_number(
    values: &ResolvedSettings,
    name: &str,
    fallback: impl FnOnce(&ResolvedSettings) -> Option<u8>,
) -> Option<u8> {
    extension_setting(values, name)
        .and_then(settings::SettingValue::as_integer)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value > 0)
        .or_else(|| fallback(values))
}

pub(super) fn terminal_caret(values: &ResolvedSettings) -> Option<settings::TerminalCaretShape> {
    extension_setting(values, "caret-shape")
        .and_then(settings::SettingValue::as_text)
        .and_then(settings::TerminalCaretShape::from_id)
        .or_else(|| values.terminal_caret_shape())
}

pub(super) fn terminal_profile(values: &ResolvedSettings) -> Option<String> {
    extension_setting(values, "default-profile")
        .and_then(settings::SettingValue::as_text)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| values.terminal_default_profile())
}

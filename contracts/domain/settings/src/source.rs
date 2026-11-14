//! 実行時に候補・既定値を供給する設定ソースの port。
//!
//! 候補や既定値が保存値だけでは決まらない設定（Host が公開する一覧など）は、
//! その機能が供給者として登録する。定義データは「供給元から取る」ことだけを
//! `OptionSource::Provided` で宣言し、設定 UI は供給元の型や状態に依存しない。
//!
//! 供給元の状態は供給する Feature が所有する。この trait はその状態を読む
//! ためだけの窓口であり、登録・解除は Client 側の設定ソース置き場が行う。

use crate::definition::SettingKey;
use crate::value::SettingValue;

/// 設定キーの候補と既定値を供給する port。
///
/// 同じキーを複数の供給元が持ってはならない。1キーの所有者は1つに限る。
pub trait SettingsSourceProvider {
    /// この供給元が扱う設定キー。定義データの `Provided` 宣言と突き合わせる。
    fn keys(&self) -> &'static [SettingKey];

    /// 選択肢と表示名。順序は供給側が決める。候補を持たないキーは空を返す。
    ///
    /// 表示名は実行時の値（フォント名・profile 名）なので翻訳キーではなく
    /// 固定文字列で返す。
    fn options(&self, key: SettingKey) -> Vec<(SettingValue, String)>;

    /// 保存値が無いときに選択状態として見せる実効既定値。
    /// 決められないキーは `None`。
    fn default_value(&self, key: SettingKey) -> Option<SettingValue>;
}

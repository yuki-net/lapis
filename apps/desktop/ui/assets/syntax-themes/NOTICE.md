# 構文配色の原典と帰属

配色を保守・配布する開発者向け。設定にはLapis / VS Code / Zed / IntelliJの4系統を表示し、それぞれのDark / Light、計8配色を内部に持ちます。明暗はアプリのDark / Whiteに常に追従し、系統だけを保存します。

`src/theme/syntax.rs`では原典の代表的な文字色を14個のSyntectスコープ規則に手動で対応づけています。テーマ全体の取り込み、背景色・フォント装飾・意味解析による分類の再現は行いません。製品公式の完全互換テーマではありません。

- **Lapis Dark / Light**: Lapisの配色。Darkは下記Dark+の基本パレットも参考にしています。
- **VS Code Dark+ / Light+**: Microsoftの[dark_plus.json](https://github.com/microsoft/vscode/blob/main/extensions/theme-defaults/themes/dark_plus.json)、[light_plus.json](https://github.com/microsoft/vscode/blob/main/extensions/theme-defaults/themes/light_plus.json)と継承元のdark_vs.json / light_vs.json。MIT、Copyright (c) 2015 - present Microsoft Corporation。ライセンス全文: [VSCode-MIT.txt](VSCode-MIT.txt)。semanticTokenColorsは取り込みません。
- **Zed One Dark / One Light**: Zed Industriesの[one.json](https://github.com/zed-industries/zed/blob/main/assets/themes/one/one.json)のstyle.syntax。テーマディレクトリ固有の[LICENSE](https://github.com/zed-industries/zed/blob/main/assets/themes/one/LICENSE)はMIT、Copyright (c) 2014 GitHub Inc.です。Zed本体のライセンスとは区別します。全文: [Zed-One-MIT.txt](Zed-One-MIT.txt)。
- **IntelliJ Dark / Light**: JetBrains IntelliJ CommunityのApache-2.0版[bb35c448e29bb5452cfcaca7c52fb0035cc80dad](https://github.com/JetBrains/intellij-community/tree/bb35c448e29bb5452cfcaca7c52fb0035cc80dad)を参照。Copyright (C) JetBrains s.r.o.。[LICENSE](JetBrains-Apache-2.0.txt)、[NOTICE](JetBrains-NOTICE.txt)を同梱しています。下記ファイルから文字色を抜粋し、Syntect用に対応づけた変更版です。
  - Dark: [expUI_darkScheme.xml](https://github.com/JetBrains/intellij-community/blob/bb35c448e29bb5452cfcaca7c52fb0035cc80dad/platform/platform-resources/src/themes/expUI/expUI_darkScheme.xml)。
  - Light: [DefaultColorSchemesManager.xml](https://github.com/JetBrains/intellij-community/blob/bb35c448e29bb5452cfcaca7c52fb0035cc80dad/platform/platform-resources/src/DefaultColorSchemesManager.xml)の`scheme name="Default"`。明示色のない分類には既定文字色を使用します。

旧6配色のIDは対応する系統として読み込み、次の保存から系統IDへ正規化します。旧DarculaはIntelliJ系統へ移行し、明暗を固定する選択肢としては残しません。不明なスコープは各配色の既定文字色を使用します。

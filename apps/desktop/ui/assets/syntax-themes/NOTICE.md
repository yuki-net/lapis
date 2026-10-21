# 構文配色の原典と帰属

配色を保守・配布する開発者向け。`src/theme/syntax.rs`では下記原典の代表的な文字色を14個のSyntectスコープ規則に手動で対応づけています。テーマ全体の取り込み、背景色・フォント装飾・意味解析による分類の再現は行いません。製品公式の完全互換テーマではありません。

- **Lapis Dark / Light**: Lapisの配色。Darkは下記Dark+の基本パレットも参考にしています。
- **VS Code Dark+**: Microsoftの[dark_plus.json](https://github.com/microsoft/vscode/blob/main/extensions/theme-defaults/themes/dark_plus.json)と継承元[dark_vs.json](https://github.com/microsoft/vscode/blob/main/extensions/theme-defaults/themes/dark_vs.json)。MIT、Copyright (c) 2015 - present Microsoft Corporation。ライセンス全文: [VSCode-MIT.txt](VSCode-MIT.txt)。semanticTokenColorsは取り込みません。
- **Zed One Dark**: Zed Industriesの[one.json](https://github.com/zed-industries/zed/blob/main/assets/themes/one/one.json)のOne Dark / style.syntax。テーマディレクトリ固有の[LICENSE](https://github.com/zed-industries/zed/blob/main/assets/themes/one/LICENSE)はMIT、Copyright (c) 2014 GitHub Inc.です。Zed本体のライセンスとは区別します。全文: [Zed-One-MIT.txt](Zed-One-MIT.txt)。
- **IntelliJ Dark / Darcula**: JetBrains IntelliJ CommunityのApache-2.0版[bb35c448e29bb5452cfcaca7c52fb0035cc80dad](https://github.com/JetBrains/intellij-community/tree/bb35c448e29bb5452cfcaca7c52fb0035cc80dad)を参照。Copyright (C) JetBrains s.r.o.。[LICENSE](JetBrains-Apache-2.0.txt)、[NOTICE](JetBrains-NOTICE.txt)を同梱しています。下記ファイルから文字色を抜粋し、Syntect用に対応づけた変更版です。
  - 新Dark: [`expUI_darkScheme.xml`](https://github.com/JetBrains/intellij-community/blob/bb35c448e29bb5452cfcaca7c52fb0035cc80dad/platform/platform-resources/src/themes/expUI/expUI_darkScheme.xml)。
  - 旧Darcula: [`DefaultColorSchemesManager.xml`](https://github.com/JetBrains/intellij-community/blob/bb35c448e29bb5452cfcaca7c52fb0035cc80dad/platform/platform-resources/src/DefaultColorSchemesManager.xml)の`scheme name="Darcula"`。
  - 比較対象: [`expUI_darculaContrastScheme.xml`](https://github.com/JetBrains/intellij-community/blob/bb35c448e29bb5452cfcaca7c52fb0035cc80dad/platform/platform-resources/src/themes/expUI/expUI_darculaContrastScheme.xml)。Darculaを継承し、背景・ガター等を新UI向けに上書きします。今回必要な一般的なkeyword/string/comment/number/function色は継承元にあり、UI色を変更しない要件に合う旧Darcula本体を採用しました。Markdown等の一部overrideは移植しません。

不明なスコープは各配色の既定文字色を使用します。色の一致のためだけの追加解析や自動視認性補正は行いません。

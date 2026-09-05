# Desktop UI Hot Reload PoC

Issue #19 のPoCは、`--hot-reload-demo` で起動できます。

## 開発時

リポジトリルートから次を実行します。

```text
cargo run -p lapis -- --hot-reload-demo
```

起動すると、メイン画面とは別に `Lapis Hot Reload Demo` ウィンドウが開きます。次のdefinitionを編集して保存してください。

- `apps/desktop/ui/assets/hot-reload/demo.toml`
- TOML parse / validationに成功したdefinitionだけを次のframeからGPUIへ渡します。
- syntax、未知のcomponent、未知のtheme tokenなどで失敗した場合は最後に成功したdefinitionを表示し続けます。
- headerのgeneration、parse時間、mtimeから観測したsave→visible時間を表示します。
- `increment` ボタンのclick数はdefinition reload後も維持されます。

定義から参照できるcomponentは `container`、`surface`、`text`、`button`、`counter`、`badge` の登録済みIDだけです。任意のRust関数やevent handlerをTOMLから実行する仕組みはありません。

開発watcherは200ms間隔のpollingで、ファイル読み取りとparse/validationはGPUIのbackground executorで実行します。render中にfilesystemやbackendへアクセスしません。

## Release

release buildではwatcherをコンパイルせず、同梱のdefinitionを同じ `UiDefinition → GPUI Div → GPUI renderer → GPU` 経路で描画します。Hot Reloadはdevelopment toolingだけを追加し、backend/core、remote protocol、`vendor/gpui` には依存しません。

RustのUI logic、component registry、action/event handler、backend契約を変更した場合は、従来どおりRust buildとアプリ再起動が必要です。このPoCがreloadするのはpresentation definition（layout、style、text、visibility、registered component composition）のみです。

## 手動確認

Windowsで次の順に確認します。

1. `cargo run -p lapis -- --hot-reload-demo` を起動する。
2. `demo.toml` の `root.layout.gap` や `root.children.children.style.width` を変更し、再起動せずに見た目が変わることを確認する。
3. `direction = "column"` または子要素の `order = 2` を変更して layout/order を確認する。
4. 一時的に `background = "token.not_registered"` を保存し、最後の成功画面とerror表示を確認する。
5. 値を戻し、generationが進み自動復帰することを確認する。
6. 20回程度保存してcrashやresource leakがないことを確認する。

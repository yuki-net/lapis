# Terminal 拡張

Terminal のセッション操作、公開 DTO、Client の画面モデルを配置する領域です。Host の共通プロトコルは Terminal の型に依存せず、汎用拡張操作とプロセス能力を提供します。Client の表示器への配線には Terminal 固有のコードが残っています。

## 構成

- `component/`: 公開 WIT を通して Host の能力を使う Wasm 実行アダプターと、表示・設定・翻訳の宣言。
- `host/`（crate `terminal-host`）: 拡張セッション操作の判断。Host 能力を port として受け取り、Wasm や Host の内部実装を参照しない。`component/` と `native/` はここを共有する。
- `native/`: 公開 native ABI v1 を使って同じロジックを接続する。Windows x86_64 の配布入力には、このライブラリと共通の設定・表示宣言を含める。
- `api/`: component とクライアントが共有するセッション操作の v1 DTO。操作名も `terminal.v1.*` で版を持つ。Host 内部型には依存しない。
- `contract/`（crate `terminal-contract`、参照名 `terminal`）: Terminal の型・wire DTO・`TerminalBackend` port の正本。
- `adapter/`（crate `local-terminal`）: `TerminalHost` port の PTY 実装と、Host 側の Terminal セッションサービス。現行 Host の配線は未接続で、一部の非 Windows テストで使う。
- `renderer/`（crate `terminal-renderer`）: Desktop のネイティブ描画器。VT/ANSI の画面状態と入力変換を担い、PTY を所有しない。
- `portable-renderer/`（crate `terminal-portable-renderer`）: Web と Wasm でもビルドできる画面モデル。ターミナル画面を Wasm で描画するものではない。Web はこのモデルをクライアント側で描画し、拡張ロジックだけが Wasm で動く。

`component/`、`host/`、`api/` は同じ親ディレクトリに置けば本体の Cargo workspace 外でも
ビルドできる。`python -m scripts.extensions.smoke_terminal` はその状態をコピーで再現し、
Wasm のビルドと PTY 操作に加え、`contract/` と `portable-renderer/` も本体の
workspace 設定なしでビルドできることを検証する。

## ディレクトリ規約の判断

役割ディレクトリで分けます。機能ごとの `src/` を平坦に並べると、契約と OS 依存実装と Host 状態のどれが正本か読み取れなくなるためです。

- `contract/` は `protocol/types/` と同じ「models」層として依存検査します。現在は Client 側の描画・入力モデルと旧 adapter が参照します。
- `host/` もセッション状態と操作判断を持つため「runtime」層として扱います。Host の能力は port 経由で利用します。
- `native/` は ABI とロジックを接続する「infrastructure」層として扱います。
- `adapter/` は `host/adapters/*` と同じ「infrastructure」層として扱います。OS API と外部プロセスに触れるためです。
- `renderer/` は PTY や Host を参照しない画面モデルとして扱います。
- 固定データを配布する必要が出たら `assets/` を追加します。現時点で Terminal は配布する固定データを持たないため作りません。

拡張の中にあっても層の意味は変えません。`scripts/check_architecture.py` は `extensions/<機能>/{contract,host,adapter}` をこの対応で分類します。

## 配線

Desktop と Web は拡張の `terminal.v1.*` 操作を汎用の `host.extensions.dispatch` 経由で呼びます。Host は必要な PTY とプロセス情報を公開能力として仲介します。Terminal の ON／OFF は Host の次回起動で反映します。

開発起動では `scripts/extensions/dev_bundle.py` が `extensions/` の宣言を発見して同梱 package を作ります。Windows x86_64 では native アダプターを使用します。

## 挙動

- タブを開くとセッションを開始し、タブを閉じると終了します。
- Workspace の切断は detach で、PTY は残ります。
- タブごとに個別のセッションを持ちます。
- `terminal.v1.profiles` で選択肢を取得し、
  `terminal.v1.start-profile` に profile ID を渡す。起動 command は拡張側で決定し、
  未知の ID を拒否する。Desktop・Web は接続ごとに同じ操作を使用する。
- フォント、行送り、カーソル、既定 profile は拡張の設定宣言から設定する。
  拡張側が未設定の場合は既存の Terminal 設定値を参照し、値がなければ描画器の既定値を使う。
  旧設定との互換値は Client に残っている。

## Client 側の載せ方

Terminal の Client UI は、共通 Layout の宣言と内部レジストリを通して載ります。

- 拡張は `views.json` で表示名、配置、複数個体、オープン用コマンドを宣言します。
- 現在の Client は宣言の View ID、表示名、既定配置、個体方針とオープン用コマンドを使用します。Client は `lapis.terminal` の `lapis.content.terminal-grid.v1` 宣言を既存のネイティブ描画器に結び付けます。他の提供元やコンテンツ種別の動的な描画器登録は未実装です。
- `clients/ui/src/features/terminal/lifecycle.rs` が `ViewLifecycle` を実装し、タブごとの Session を管理します。表示器は Desktop と Web の Client 内で動きます。
- 設定への候補供給（フォント・既定 profile）は `SettingsSourceProvider` として登録します。設定画面は Terminal Feature の型を読まず、登録された供給元をキーで引きます。

## 検証

- `cargo test -p terminal-contract -p terminal-host -p terminal-renderer -p local-terminal`
- `python scripts/check_architecture.py`
- `python -m unittest scripts.ci.test_paths`

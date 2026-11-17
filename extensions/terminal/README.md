# Terminal 拡張

Terminal の契約・Host 側状態・PTY 実装を配置する領域です。Client UI や Host の配線には Terminal 固有のコードが残っており、拡張 API を使った独立実行への移行は未完了です。

## 構成

- `component/`: 公開 WIT のみで Host の PTY を利用する独立 Cargo workspace。生データとセッション操作を扱う。Desktop・Web に試験用の接続があり、release の同梱入力にも含まれる。通常経路への移行は未完了。
- `api/`: component とクライアントが共有するセッション操作の v1 DTO。操作名も `terminal.v1.*` で版を持つ。Host 内部型には依存しない。
- `contract/`（crate `terminal-contract`、参照名 `terminal`）: Terminal の型・wire DTO・`TerminalBackend` port の正本。
- `host/`（crate `terminal-state`）: Host 側の正規状態、画面差分、接続 Session。`TerminalRegistry` がこれを持つ。
- `adapter/`（crate `local-terminal`）: OS の PTY を使う `TerminalBackend` 実装。
- `renderer/`（crate `terminal-renderer`）: VT/ANSI の画面状態と入力変換。PTY の所有と分けたネイティブ描画器で、現在の実装は Wasm に未対応。
- `portable-renderer/`（crate `terminal-portable-renderer`）: Web と Wasm でもビルドできる画面モデル。ターミナル画面を Wasm で描画するものではない。

`component/` と `api/` は同じ親ディレクトリに置けば本体の Cargo workspace 外でも
ビルドできる。`python -m scripts.extensions.smoke_terminal` はその状態をコピーで再現し、
Wasm のビルドと PTY 操作に加え、`contract/` と `portable-renderer/` も本体の
workspace 設定なしでビルドできることを検証する。

## ディレクトリ規約の判断

役割ディレクトリで分けます。機能ごとの `src/` を平坦に並べると、契約と OS 依存実装と Host 状態のどれが正本か読み取れなくなるためです。

- `contract/` は `contracts/` と同じ「models」層として依存検査します。Client と Host の両方から参照するためです。
- `host/` は `host/runtime` と同じ「runtime」層として扱います。正規状態と操作判断を持ち、UI を持たないためです。
- `adapter/` は `host/adapters/*` と同じ「infrastructure」層として扱います。OS API と外部プロセスに触れるためです。
- `renderer/` は PTY や Host を参照しない画面モデルとして扱います。
- 固定データを配布する必要が出たら `assets/` を追加します。現時点で Terminal は配布する固定データを持たないため作りません。

拡張の中にあっても層の意味は変えません。`scripts/check_architecture.py` は `extensions/<機能>/{contract,host,adapter}` をこの対応で分類します。

## 配線

Host は build 時にこの拡張をリンクして注入します。

- `host/runtime`（`backend-state`）は `terminal-state` を path 依存で参照します。
- `host/adapters/local`（`platform`）は `local-terminal` を参照し、`LocalTerminalBackend` と既定 profile を再エクスポートします。`host/app` はこれを組み立てます。
- `host/adapters/terminal` と `host/terminal` は削除済みです。旧位置に re-export を残すと「どちらが正本か」が読めなくなるためです。

workspace は `Cargo.toml` の `extensions/*/*` glob で `contract` / `host` / `adapter` / `renderer` を拾います。

## 挙動

- タブを開くとセッションを開始し、タブを閉じると終了します。
- Workspace の切断は detach で、PTY は残ります。
- タブごとに個別のセッションを持ちます。
- 試験用の拡張経路では `terminal.v1.profiles` で選択肢を取得し、
  `terminal.v1.start-profile` に profile ID を渡す。起動 command は拡張側で決定し、
  未知の ID を拒否する。Desktop・Web は接続ごとに同じ操作を使用する。

## Client 側の載せ方

Terminal の Client UI は、共通 Layout の宣言と内部レジストリを通して載ります。

- 宣言（`clients/ui/src/features/terminal/mod.rs` の `descriptor()`）が、BottomDock への View、`multiple_instances`、`references_external_resource` を供給します。タブ単位の Session はこの宣言で個体化されます。
- 個体と Session の所有者（`clients/ui/src/features/terminal/lifecycle.rs`）が `ViewLifecycle` を実装し、`FeatureRegistry` へ登録します。Shell はターミナル固有の分岐を持たず、Layout 上のタブと表示状態だけを見て個体の生成・表示の反映・終了を委ねます。
- 設定への候補供給（フォント・既定 profile）は `SettingsSourceProvider` として登録します。設定画面は Terminal Feature の型を読まず、登録された供給元をキーで引きます。

## 検証

- `cargo test -p terminal-contract -p terminal-state -p terminal-renderer -p local-terminal`
- `python scripts/check_architecture.py`
- `python -m unittest scripts.ci.test_paths`

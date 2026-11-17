# Terminal 拡張

Terminal の契約・Host 側状態・PTY 実装を配置する領域です。Client UI や Host の配線には Terminal 固有のコードが残っており、拡張 API を使った独立実行への移行は未完了です。

## 構成

- `component/`: 公開 WIT のみで Host の PTY を利用する独立 Cargo workspace。生データとセッション操作を扱う。既存 UI への接続は未完了で、出荷対象にはまだ含めない。
- `contract/`（crate `terminal-contract`、参照名 `terminal`）: Terminal の型・wire DTO・`TerminalBackend` port の正本。
- `host/`（crate `terminal-state`）: Host 側の正規状態、画面差分、接続 Session。`TerminalRegistry` がこれを持つ。
- `adapter/`（crate `local-terminal`）: OS の PTY を使う `TerminalBackend` 実装。VT/ANSI の解釈もここに閉じる。

## ディレクトリ規約の判断

役割ディレクトリで分けます。機能ごとの `src/` を平坦に並べると、契約と OS 依存実装と Host 状態のどれが正本か読み取れなくなるためです。

- `contract/` は `contracts/` と同じ「models」層として依存検査します。Client と Host の両方から参照するためです。
- `host/` は `host/runtime` と同じ「runtime」層として扱います。正規状態と操作判断を持ち、UI を持たないためです。
- `adapter/` は `host/adapters/*` と同じ「infrastructure」層として扱います。OS API と外部プロセスに触れるためです。
- 固定データを配布する必要が出たら `assets/` を追加します。現時点で Terminal は配布する固定データを持たないため作りません。

拡張の中にあっても層の意味は変えません。`scripts/check_architecture.py` は `extensions/<機能>/{contract,host,adapter}` をこの対応で分類します。

## 配線

Host は build 時にこの拡張をリンクして注入します。

- `host/runtime`（`backend-state`）は `terminal-state` を path 依存で参照します。
- `host/adapters/local`（`platform`）は `local-terminal` を参照し、`LocalTerminalBackend` と既定 profile を再エクスポートします。`host/app` はこれを組み立てます。
- `host/adapters/terminal` と `host/terminal` は削除済みです。旧位置に re-export を残すと「どちらが正本か」が読めなくなるためです。

workspace は `Cargo.toml` の `extensions/*/*` glob で `contract` / `host` / `adapter` を拾います。

## 挙動

- タブを開くとセッションを開始し、タブを閉じると終了します。
- Workspace の切断は detach で、PTY は残ります。
- タブごとに個別のセッションを持ちます。

## Client 側の載せ方

Terminal の Client UI は、共通 Layout の宣言と内部レジストリを通して載ります。

- 宣言（`clients/ui/src/features/terminal/mod.rs` の `descriptor()`）が、BottomDock への View、`multiple_instances`、`references_external_resource` を供給します。タブ単位の Session はこの宣言で個体化されます。
- 個体と Session の所有者（`clients/ui/src/features/terminal/lifecycle.rs`）が `ViewLifecycle` を実装し、`FeatureRegistry` へ登録します。Shell はターミナル固有の分岐を持たず、Layout 上のタブと表示状態だけを見て個体の生成・表示の反映・終了を委ねます。
- 設定への候補供給（フォント・既定 profile）は `SettingsSourceProvider` として登録します。設定画面は Terminal Feature の型を読まず、登録された供給元をキーで引きます。

## 検証

- `cargo test -p terminal-contract -p terminal-state -p local-terminal`
- `python scripts/check_architecture.py`
- `python -m unittest scripts.ci.test_paths`

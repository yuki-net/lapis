# Desktop Client

- `app/` は起動・終了と組み立て、`ui/shell/` は Window・Panel・Tab・Layout。
- `ui/features/` は各機能の表示状態・入力・描画、components/theme/devtools は共通 UI。
- 正規状態・実行資源を UI に所有させない。操作と購読は共通 Client Adapter を通す。
- 描画・UI スレッドで同期 I/O をしない。非同期結果は対象・世代・revision を確認する。
- 機能横断の `#[path]` 取り込みや巨大ルートへの可変状態集約を追加しない。
- 依存方向と状態所有の正本は `docs/specs/architecture.md`。

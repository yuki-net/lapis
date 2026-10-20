# Desktop

Rust/GPUIによるDesktop Clientの実装領域です。現在のファイル配置やEntity分割はプロトタイプであり、今後の設計変更を妨げる規則ではありません。

## 役割と理由

- Window、Panel、focus、layoutを扱うShellと、個別の機能の状態を混同しない。全機能の可変状態を巨大なShellへ集約しないためです。
- 描画は現在の状態からUI要素を作ることに集中し、Render中の外部I/OやUIスレッドのblockingを避ける。描画の応答性を守るためです。
- Editorの入力や選択など即時性が必要な表示状態を、不必要な外部応答待ちにしない。
- 非同期の結果は、対象の状態や世代を確かめてから適用する。古い結果による画面の巻き戻りを防ぐためです。
- GPUI固有の型やEventをバックエンドの共通契約へ漏らさない。

必要な境界は状態の所有者やライフサイクルで判断し、行数のためだけのEntity・ファイル分割をしません。

UIの共通の意図は [Design](../../docs/DESIGN.md)、手動検証の扱いは [AGENTS.md](../../AGENTS.md) を参照してください。

## ディレクトリの責務

- `app/` は Desktop の起動・終了と依存注入を行う Composition Root。Backend の具体実装を組み立てるのはここに限定します。
- `client/` は Backend の表示 cache・Client 操作・復元用表示状態を所有し、GPUI や具体的な OS 実装には依存しません。
- `persistence/` は Desktop の Conversation・Window 表示状態・設定を保存します。Backend の実行資源を復元・再起動する責務は持ちません。
- `ui/shell/` は Window・Panel・Tab・Layout、`ui/features/` は個別機能の表示・入力・描画を担当します。
- UI に Backend の正規状態や実行ロジックを移さず、機能横断的な `#[path]` 取り込みは禁止します。
- 依存方向の詳細は [責務と依存方向](../../docs/specs/architecture.md) を参照してください。

## Window の所有と検証

Root ごとに WindowGroup を作り、Detached は元の Client worker と Document を共有する。Shell の共通描画を使い、Focus・Active Tab・scroll は Window ごとに持つ。Tool の実行資源を複製して移動することはない。終了・移動・将来の配置復元の契約は [WindowGroup](../../docs/specs/architecture.md#desktop-の-windowgroup) を参照。

Windows では `cargo build -p lapis` 後に `./scripts/test_window_host.ps1` で隔離した Native Window のスモーク検証を実行できる。Release は `-Binary <release/lapis.exe>` を指定する。テストは一時 Workspace のみを編集し、利用者の設定は使用しない。

Document タブは ↗ / ↩ ボタンで取り外し・Root へ再ドックできる。Windows ではタブを Window 外で離すと取り外し、同じグループの別 Window の Panel 上で離すと再ドックする。Escape で取り消す。移動で空になった子 Window は手動で閉じる。Tool の Window 間移動と、再起動後の Window 配置復元は未対応。

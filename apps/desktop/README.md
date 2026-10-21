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

## Host接続

通常起動は独立したLocal HostへQUICで接続します。「ファイル → Hostに接続…」は招待または保存済みprofileを選び、新しい接続画面を開きます。既存画面の未保存編集は保持します。起動時は `--host <file>`、検証用に状態を分離する場合は `--state-directory <directory>` を使用できます。

既定の保存先を変更した環境でも、旧保存先にだけdraft・会話・設定がある場合は元の場所を継続使用します。新旧両方に状態がある場合は新しい保存先を使用し、旧保存先を標準エラーへ通知します。自動で混合・上書きせず、必要な保存先は `--state-directory` で選択できます。

Remoteのファイル操作はHostのWorkspaceを使用し、Client上に同じsource folderを必要としません。接続先ごとに画面状態を保存します。Remote向けのLSPと検索は未接続です。招待の発行・期限・保護方法は [Backend](../../backend/README.md) を参照してください。

Remoteの「Open」「Save As」と未保存文書の保存は、Workspaceからの相対pathを入力します。保存先が競合すると変更を保持したまま拒否します。ファイルの通常保存はCtrl+S、名前を付けて保存はCtrl+Shift+Sです。

Terminalの「Host Terminals」は既存端末を一覧し、「Attach」で同じprocessへ接続します。Shell / PowerShell / cmdから起動でき、入力・Ctrl+C・矢印・貼り付けを送れます。サイズ変更は共有端末全体へ反映され、終了は確認後に全Clientへ反映されます。Task切替・画面終了ではprocessを終了しません。前回保存したTerminal IDはworkerから再アタッチし、shellを再実行しません。
## Window の所有と検証

Root ごとに WindowGroup を作り、Detached は元の Client worker と Document を共有する。Shell の共通描画を使い、Focus・Active Tab・scroll は Window ごとに持つ。Tool の実行資源を複製して移動することはない。終了・移動・将来の配置復元の契約は [WindowGroup](../../docs/specs/architecture.md#desktop-の-windowgroup) を参照。

Windows では `cargo build -p lapis` 後に `./scripts/test_window_host.ps1` で隔離した Native Window のスモーク検証を実行できる。Release は `-Binary <release/lapis.exe>` を指定する。テストは一時 Workspace のみを編集し、利用者の設定は使用しない。

Document タブは ↗ / ↩ ボタンで取り外し・Root へ再ドックできる。Windows ではタブを Window 外で離すと取り外し、同じグループの別 Window の Panel 上で離すと再ドックする。Escape で取り消す。移動で空になった子 Window は手動で閉じる。Tool の Window 間移動と、再起動後の Window 配置復元は未対応。

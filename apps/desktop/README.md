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
- 左のProject / Taskで既存フォルダをProjectへ登録し、登録先からPrimaryを一つ、Subを任意で一つ選んで作業Taskを作成します。Git管理されていない既存ソースフォルダも選べます。
- 作業Taskの選択・Primary/Sub・文書とPanelの復元参照はDesktopの保存状態です。既存のConversation IDを復元キーに再利用し、HostのAgent実行Task/Executionとは区別します。切替ではHostの文書・PTY・CLIを終了しません。Worktreeの作成・カスタマイズはGit操作として扱います。
- Project / Taskの保存状態が壊れている場合はエラーを表示して書き戻しを拒否します。従来の会話はrootごとのProjectへ移行し、Project未所属の下書きはTask外の復元項目として残します。
- UI に Backend の正規状態や実行ロジックを移さず、機能横断的な `#[path]` 取り込みは禁止します。
- 依存方向の詳細は [責務と依存方向](../../docs/specs/architecture.md) を参照してください。

## 開発コマンド

Desktopを変更・確認する開発者向け。以下は対象のworktreeルートで実行します。

| 操作 | コマンド |
| --- | --- |
| ビルドして起動 | `cargo dev` |
| ビルドのみ | `cargo dev-build` |
| ファイルを開いて起動 | `cargo dev --open-file README.md` |
| DesktopのCIと同じ検証 | `python scripts/check.py desktop` |
| BackendのCIと同じ検証 | `python scripts/check.py backend` |
| 特定の検証のみ | `python scripts/check.py desktop test` |

IDEの `.run/Lapis.run.xml` も `cargo dev` を使います。Lapis内で作成したworktreeのTerminalからも、そのworktreeルートで同じコマンドを実行できます。単体の回帰テストは通常の `cargo test -p <package> <filter>` で実行します。Mobile / Webの既存コマンドは変更しません。

build/runは同じ `target/` を使い、開発用の設定・会話・復元状態・Host接続情報は `target/dev-state/` に保存します。他のworktreeや通常起動の保存先とは共有しません。アプリとHostの処理は通常版と同じで、起動引数だけが異なります。明示的に同じファイルやRemote Hostを開いた場合、その実体の共有まで防ぐ仕組みではありません。新しい保存先に既存の個人設定を自動コピーしません。

画面は通常どおり閉じるかIDEで停止します。IDEの強制停止では未保存状態の確定を保証しないため、編集がある場合は先に保存して画面を閉じてください。Local Hostは製品仕様どおり画面終了後も存続します。Windowsで再ビルド・worktree削除がexeの使用中エラーになる場合は、画面と作業プロセスを終了してから `powershell -NoProfile -File scripts/stop_dev_host.ps1` を実行します。この明示操作は該当worktreeのLocal Hostを終了するため、Host上の未保存Document・Terminal等の実行状態は失われます。他のworktreeのexeは停止しません。

生成物と開発用状態はGit管理外の `target/` 内に置くため、worktreeのディレクトリ削除と一緒に消えます。`cargo clean`でも開発用状態ごと削除されるため、残したいファイルは先に保存してください。worktree削除前に起動中のプロセスを終了し、復元用の下書きも必要なら保存します。以前の `target/run/` は自動移動・削除しません。不要なら停止後に削除できます。Cargoのダウンロードキャッシュはユーザー共通で再利用します。

コミット時のhookはRustのfmtとworkspace全体の`cargo check`で、変更関連のテストは手元で実行します。全体の合否は `.github/workflows/ci.yml` のCIで確認します。Rustの検証内容は `scripts/check.py` をCIとローカルで共有し、既存の対象・警告の扱いを維持しています。実画面の確認はCIとは別に行います。

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

# 責務と依存方向

機能を追加・移行する開発者が、状態の所有者と配置先を判断するための正本。

| 領域 | 所有するもの | 所有しないもの |
| --- | --- | --- |
| `apps/` | Client の Window、Panel、Tab、focus、selection、表示 cache | Host 上の実行資源 |
| `shared/models/` | Transport 非依存の値・ドメイン規則・port | GPUI、通信、外部プロセスの具体実装 |
| `shared/protocol/` | request/response/event、revision、認可可能な操作契約 | OS 実装、Client の画面構成 |
| `backend/application/` | 操作と判断、port による外部連携 | GPUI、OS 固有 API |
| `backend/runtime/` | 正規状態、操作の直列化、購読・revision・実行資源の寿命 | Window、Panel、focus |
| `backend/infrastructure/` | FS、Git CLI、PTY、LSP、runner、永続化 | 画面の判断 |
| `backend/transport/` | 認証・wire・接続と共通 Runtime への配送 | 独立した Workspace/Document 正本 |
| `backend/host/` | Runtime と具体実装の組み立て、開始・終了 | UI |

## 配置の判断

状態とリソースの所有原則は [全体アーキテクチャ](../ARCHITECTURE.md) を参照する。追加実装の具体的な配置は次のとおり。

1. 値・規則・port は models、通信に公開する契約は protocol、操作判断は application。
2. 外部環境に触れる具体実装は infrastructure。認証や wire は transport。
3. 実際の所有・寿命の境界がなければモジュールで分割する。空 crate や転送だけの層を作らない。

## Runtime と接続の契約

- Host が登録した Workspace ごとに Runtime が Document の内容・revision・undo/redo、Terminal の出力・状態、Git/Task の操作範囲を所有する。Client の Document は表示 cache であり、編集は revision を指定して正本へ送る。
- 埋め込み接続と Remote は同じ request/response/event と Runtime を使う。Transport は認可と配送を担い、別の正規状態を作らない。
- Git/Task の外部 I/O は正規状態 worker の外で実行する。Runtime 内で接続・Workspace を検証し、Workspace ごとの実行資源を直列化したうえで、結果とイベントを worker に戻す。他 Workspace の操作や Document・Terminal の更新を Git の完了待ちにしない。
- Git/Worktree/Task の公開操作は Workspace・Worktree・Execution の ID を指定する。実行ディレクトリ、内部 TaskRecord、OS の絶対パスを wire に公開せず、実行 ID と初期状態は Runtime が生成する。
- Document の close や接続切断は attachment の解除であり、未保存内容・履歴・Terminal・Agent の終了ではない。実行終了には明示的な終了操作が必要。
- Document の古い revision は拒否する。保存済みの Client 表示状態を復元しても、すでに存在する正規 Document の新しい内容を上書きしない。
- 重なる Workspace grant から同じ canonical path を別の Document として開く操作は競合として拒否する。別 Workspace の内容を暗黙に共有して認可範囲を広げない。
- Terminal の購読欠落は snapshot と sequence で回復する。一時的な poll・通信失敗を実行終了とみなさず、出力の置換 snapshot と追記イベントを区別する。

## Desktop の更新境界

- Root は Entity の組み立てと機能間イベントの接続を担当する。Shell は Window・Panel・Tab・Layout を所有し、各 Feature の Backend 操作や表示 cache を集約しない。
- Feature は自身の入力・表示 cache・更新周期を所有する。共通 Client Adapter の worker で操作を直列化し、描画・入力処理から FS・Git・PTY や同期 port を直接呼び出さない。
- 応答の適用には要求順序と対象の世代を検証する。楽観編集の先行操作が失敗したら、数値 revision が偶然一致しても依存する後続編集を適用しない。未確定入力は明示的な復旧まで保持する。
- Window 終了時は受付済み入力と表示状態の保存を完了させる。購読・表示 worker の破棄によって Host の Terminal・Agent を終了しない。保存先や Workspace が失われても、復元用 draft を無言で破棄しない。

## Desktop の WindowGroup

- Root ごとに独立した WindowGroup と Client worker を持つ。Detached は元の worker・Document・Conversation を共有し、Feature や実行サービスを作り直さない。別 Root のグループへタブを渡さない。
- 共通の OS Window options と Shell が、テーマ・タイトルバー・Window Controls・タブ・フッターを描画する。Detached は Root 用のメニューや補助 Panel を持たない。
- WindowGroup は Window の親子関係、`PanelTab` の所属 Window・Panel、終了中フラグを持つ。Window bounds はイベントで更新する。Active Tab・focus・selection・scroll・Panel サイズは各 Window の Shell / Editor が所有する。
- Document の操作は共有 worker で直列化し、送信元 Window の対象を操作前に選ぶ。新規 Document の作成元も worker 内で記録し、先に届いた Snapshot の送信元から所属を推測しない。応答の連番と各 Editor の世代で古い結果を拒否する。移動中に入力を失わないため、未完了入力・復旧待ち・終了処理中の移動は受け付けない。
- Window の生成成功後にタブの所属を移す。生成失敗・未受理のドロップでは元のタブを残す。新 Host の購読が有効になる前に初回 Snapshot と配置通知が併合される場合も、defer した通知で配置を渡す。
- close は入力の受付停止 → FIFO 上の入力失敗検査 → 全対象の保存/破棄判断 → 保存 → revision の事前検証 → タブの close → 表示状態の保存 → OS Window の閉鎖の順。Cancel / Save As のキャンセルではタブを閉じない。通常の Root close はその系列だけ、Quit は全 Root 系列を対象とし、判断が揃う前に一部だけ閉じない。外部 I/O の commit 失敗まで rollback できる分散トランザクションではない。
- Detached の終了はタブを閉じ、Root へ戻さない。表示 Entity・購読の解放と Host 上のプロセス終了は別操作である。

最初の実装では Document の取り外しボタンと Root への再ドックを対象とする。配置モデルは Document / Tool 共通だが、Tool の Window 間移動は未有効。Terminal は focus と入力先・出力購読、Task / Agent は選択 execution とストリームの寿命を持つため、既存 Entity を再利用して再購読範囲を検証するまで移動させない。個別の Window Manager や汎用 Trait は増やさない。

GPUI 0.2.2 の Windows backend は MouseDown で `SetCapture`、MouseUp で `ReleaseCapture` し、GPUI は受信した Window の MouseUp 後に active drag を破棄する。別 Window の `on_drop` だけでは OS Window 間ドロップの成立を保証できない。現状は group identity を検証する受信経路までとし、Window 外へのドラッグ取り外し・Windows 上のドラッグ再ドックは後続で扱う。GPUI の変更は加えない。

WindowGroup の再起動復元は未実装。後続では group / window の永続 ID、親子関係、bounds、Panel ごとのタブ順・Active Tab・サイズを、Document の復元用 ID と関連づけて保存する。現状の HostId / GPUI EntityId を永続 ID に使わない。明示 close 後の更新済み Document 一覧を保存し、閉じたタブ・Window を復活させない。既存の文書復元は OS Window の配置復元を意味しない。

`scripts/test_window_host.ps1` は隔離した Workspace と設定で Native Window の生成・共有編集・再ドック・グループ分離・終了・再生成と CPU の短時間サンプルを検証する。GUI の描画、保存確認ダイアログ、ドラッグ、IME の実操作は別途確認する。

## 依存の検査

`python scripts/check_architecture.py` は `cargo metadata --locked --no-deps` の全 workspace member と全 dependency（dev/build/target 条件を含む）を検査する。
許可行列は `scripts/check_architecture.py` が実行上の正本。models は models のみ、protocol は models、application は models、runtime は application/models/protocol、infrastructure は application/runtime/models、transport は runtime/protocol/models へ依存できる。各層内の依存も許可する。
Host と Desktop の `apps/desktop/app/` は Composition Root として具体実装を組み立てる。通常の Client (`apps/cli/` 等) は models/protocol への依存に限定し、Backend の実装に直接依存させない。Desktop UI は `apps/desktop/client/` と models/protocol のみへ依存できる。client は models/protocol、Desktop 表示状態を保存する persistence は client/models のみへ依存できる。Backend からこれらの Client 実装への依存は dev-dependency も禁止する。
Backend と共通契約から GPUI/native dialog への依存は禁止。

既存違反は `scripts/architecture-exceptions.json` の完全一致した crate 対と理由に限定する。未使用例外も失敗するため、削除した違反を再導入する余地を残さない。新規違反を「移行中」として追加しない。

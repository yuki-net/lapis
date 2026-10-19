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

1. プラットフォーム固有の表示状態なら Client。複数の Client に一致して見える状態なら Runtime。
2. 値・規則・port は models、通信に公開する契約は protocol、操作判断は application。
3. 外部環境に触れる具体実装は infrastructure。認証や wire は transport。
4. 実際の所有・寿命の境界がなければモジュールで分割する。空 crate や転送だけの層を作らない。
5. Window/Panel/Conversation の破棄や接続切断は実行終了を意味しない。明示的な終了操作と区別する。

## 依存の検査

`python scripts/check_architecture.py` は `cargo metadata --locked --no-deps` の全 workspace member と全 dependency（dev/build/target 条件を含む）を検査する。
許可行列は `scripts/check_architecture.py` が実行上の正本。models は models のみ、protocol は models、application は models、runtime は application/models/protocol、infrastructure は application/runtime/models、transport は runtime/protocol/models へ依存できる。各層内の依存も許可する。
Host と Desktop の `apps/desktop/app/` は Composition Root として具体実装を組み立てる。通常の Client (`apps/cli/` 等) は models/protocol への依存に限定し、Backend の実装に直接依存させない。Desktop UI は移行中のため application/models/protocol に依存できるが、具体実装や transport を直接操作しない。
Backend と共通契約から GPUI/native dialog への依存は禁止。

既存違反は `scripts/architecture-exceptions.json` の完全一致した crate 対と理由に限定する。未使用例外も失敗するため、削除した違反を再導入する余地を残さない。新規違反を「移行中」として追加しない。

## 移行の現在地

初期の配置変更だけでは正規状態の統一を意味しない。application/services の Session は Desktop の表示・操作と Backend の処理を含む移行対象。Task runner の契約とプロセス実装も同居している。状態統合・UI 分離の変更でこれらを解消する。現時点の動作契約はコードとテストを参照する。

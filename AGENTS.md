# AGENTS.md — Lapis

Lapis の構造を守るための憲法。作業手順は [docs/workflow.md](docs/workflow.md)、
全体の判断原則は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)、内部契約は
[docs/specs/README.md](docs/specs/README.md) を正本とする。

## 所有権モデル

- **BYO-Host**: 実行環境は利用者が用意・所有する Host であり、中央集約サーバーを前提にしない。
- Client（workbench / apps）は自分の **Home Host** にだけ接続する。他の Host や環境へ直接接続しない。
- 他の環境は **ExecutionTarget** として Home Host が仲介する。Client の操作は Home Host を経由して対象へ届く。
- Host は正規状態と実行資源（Workspace / Document / Terminal / Task / プロセス / ファイル / Git / Agent）を所有する。Client は表示状態と cache だけを所有する。
- Local / Remote で所有権と操作の意味を変えない。接続の切断は attachment の解除であり、実行資源の終了ではない。
- 状態の更新入口を一意にし、複数の正本を作らない。

## 依存の向き

- **protocol** は境界を越える DTO と操作契約だけを持ち、他の柱の実装・OS・UI・業務判断に依存しない。
- 実装は契約に依存する。契約が実装に依存せず、循環依存を作らない。
- **host** は protocol と extensions の Host 側実装に依存できる。workbench / apps に依存しない。
- **workbench** は protocol、共通 UI kit、extensions の Client 側契約に依存できる。host / apps に依存しない。
- **extensions** は protocol と共通 UI kit に依存できる。host / apps に依存しない。機能を垂直に閉じる。
- **apps** は Composition Root として全柱を組み立てる。どの柱からも apps へ依存しない。
- **tools** は製品の実行時依存にしない。製品コードから tools へ依存しない。
- 新しいクレート間依存は `scripts/dependency-allowlist.json` に追加し、理由をレビューで説明する。

## 6本柱の配置

| 柱 | 置くもの | 置かないもの |
| --- | --- | --- |
| apps | 起動 entry、Composition Root、target 固有の初期化 | 機能ロジック、正規状態、表示状態 |
| workbench | Window / Panel / Tab / Layout、表示状態と cache、共通 UI kit | 実行資源の正規状態、OS 実装 |
| extensions | 機能の垂直統合（契約、Host 側状態、adapter、Client UI、assets） | 他機能の内部、将来像だけの共通層 |
| host | 正規状態、操作の直列化、実行資源、永続化、通信境界 | UI、Window / Panel、表示 cache |
| protocol | 境界を越える DTO、操作契約、拡張 API | 実装、OS 依存、UI、業務判断、非同期処理 |
| tools | 開発・検証・配布のツール（devtools、wasm-smoke、scripts） | 製品の実行時依存になるコード |

- 機能追加は 6 本柱のいずれか 1 つを主な置き場所とし、層ごとに部品を散らさない。
- 柱をまたぐ移動は「がわの移動」と「中身の refactor」を同じ PR に混ぜない。
- 移行完了までの現行ディレクトリとの対応は [docs/specs/architecture.md](docs/specs/architecture.md) に従う。
- 変更時は `python scripts/check.py <surface> architecture` で構造と依存ホワイトリストを検査する。

# 自動開発フロー（automation）

## この文書の役割

Issue から実装・レビュー・マージまでを自動化するときの、**役割・境界・状態遷移・秘密の扱い**を決める正本。
実行環境は**この Windows ホスト**で、VM（Hyper-V / WSL2）は使わない。
実装（M2 以降）はこの文書と `automation/` の policy / roles に従う。

## 目的

- 人間は Issue を別の場所で作り、**「#N を作って」と明示したときだけ**開始する
- worker が実装して PR を作り、reviewer が **PR ごとに新しいセッション**でレビューする
- マージは**人間（またはオーナーが明示したとき）だけ**が行う
- 人間は**例外だけ**見る（保護パス、外部 PR、NG、リトライ上限）
- 監視は dashboard / ログ / Orca。**AI に監視させない**
- 仕様・skill・policy は repo に残す。秘密は repo に置かない

## 非目標

- 全 Issue の自動 pickup（Issue ができただけでは動かさない）
- AI による auto-merge
- AI による監視・通知の判断
- 保護パスの AI 変更
- VM / コンテナを作ること

## 役割

| 役割 | 実体 | できること | しないこと |
| --- | --- | --- | --- |
| **人間** | あなた | Issue 作成、`#N を作って`、**merge**、保護パスの承認 | — |
| **coordinator** | オーナー側 Orca bot | 監視、オーナーの疑問解決、`dispatch.py` の起動 | 実装、レビュー、merge、approve、保護パス変更、コード編集 |
| **worker** | OpenCode（Orca worktree） | 実装、push、PR 作成 | merge、保護パス変更、オーナーの token の使用 |
| **reviewer** | OpenCode（PR ごとに新規セッション） | PR のレビュー、所見の報告 | **GitHub への書き込み**、merge、保護パス変更 |

- worker と reviewer は **別 GitHub App・別 token**
- reviewer は **read-only**（`contents: read` / `pull requests: read` / `checks: read` / `commit statuses: read`）
- coordinator は監視と質問対応だけで、コードを書かない
- オーナーの `gh` ログイン（keyring）は worker / reviewer に渡さない

## 実行環境（この Windows ホスト）

- Hyper-V / WSL2 の VM は作らない
- Windows の**別ローカルユーザー `orcaworker`** で `orca serve` を起動し、オーナーの Orca に pair する
- worker / reviewer は `orcaworker` の Orca worktree / セッションで動く
- オーナーの `gh` keyring は `orcaworker` から読めない（これが境界）
- 追加の防御として、将来はサービスアカウント + localhost ブローカー（B）を重ねられる
- worker: `orca worktree create --environment orcaworker --name issue-N --agent opencode --prompt "<Issue>"`
- reviewer: PR ごとに新しいセッション。**PR が open の間は保持し、merge / close で解放**する

## 認証

- GitHub App の private key で JWT を作り、installation token（1 時間）を発行する
- **ブローカー**が `orcaworker`（B を重ねる場合はサービスアカウント）で動き、`gh` / `git` の呼び出しごとに子プロセスへ token を渡す
- token はプロセスの環境変数・ログ・ファイルに残さない
- `GH_CONFIG_DIR` はロールごとに分ける。オーナーの keyring は使わない
- token は**残り 5 分で再発行**する

## フロー

```text
人間: Issue を作成
  ↓
人間: 「#N を作って」
  ↓
coordinator: automation/dispatch.py --issue N
  - Issue の確認（open、受け入れ条件）
  - ai-building ラベル（coordinator の token）
  - ローカルロック
  - orca worktree create --environment orcaworker --name issue-N --agent opencode --prompt "<Issue>"
  ↓
worker: 実装 → push → PR
  ↓
reviewer: PR ごとに新しいセッション
  - diff / CI / policy を確認
  - 所見を dashboard / Orca に返す（GitHub には書かない）
  ↓
人間: 所見と CI を確認して merge（またはオーナーが明示）
  ↓
merge / close で reviewer セッションを解放
```

coordinator の dispatch は起動だけを行う。実装・レビュー・merge はしない。

## 発火

- **Issue ができただけでは動かさない**。人間の「#N を作って」がトリガー
- `automation/dispatch.py --issue N` が決定的に行う:
  1. Issue が open で、受け入れ条件が書かれている
  2. `ai-building` が付いていない
  3. ローカルロック（`%LOCALAPPDATA%\lapis\locks\`）が取れる
  4. `ai-building` を付ける（coordinator の token）
  5. `orca worktree create --environment orcaworker --name issue-N --agent opencode --prompt "<Issue>"` で worker を起動
- 二重起動・競合はラベルとロックで防ぐ
- 異常終了に備え、`ai-building` は heartbeat と `stale_building_minutes` で回収する

## worker

- worker App の installation token を使う。**オーナーの token は使わない**
- token はブローカーが発行し、子プロセスにだけ渡す
- 権限は `contents: write` / `pull requests: write` / `checks: read` / `commit statuses: read`
- `issues: write` と `workflows: write` は付けない。ラベルは coordinator が付ける
- CI は App の `checks: read` / `commit statuses: read` で読む
- OpenCode を基本、Junie CLI を fallback にする
- 作業後に PR を作る。PR の本文に Issue をリンクする
- 失敗の扱い:
  - 連続 2 回で `ai-needs-human`（coordinator が付ける）
  - 60 分で timeout
  - 保護パスを触った場合は人間の承認が必要
- worker は merge しない

## reviewer

- PR ごとに**新しい OpenCode セッション**を開く。前の PR の文脈を持ち越さない
- セッションは PR が open の間は保持し、merge / close で解放する
- オープン PR を**決定的に** poll する（AI の判断ではない）
- 正本は **PR の最新 HEAD SHA**
  - 最新 SHA に対する結果がなければレビュー
  - 結果があればスキップ
- 並列は 2〜4。PR ごとに claim して二重レビューを防ぐ
- push されたら古い判定を捨て、新しい SHA で再レビュー
- **GitHub には書き込まない**。verdict はローカルの dashboard / Orca に残す
- レビュー対象: diff、最新 SHA の CI（`checks: read` / `commit statuses: read` で読む）、`automation/policy.toml`、Issue / PR の本文
- コードは実行しない
- reviewer 自身の PR はレビューしない（別の reviewer か人間）

## マージ

**自動マージはしない。** merge は人間（またはオーナーが明示したとき）だけが行う。

- CI の `Gate` が green であること
- reviewer の所見が approve であること
- 保護パスを触っていないこと（触った場合は `reviews` の承認）
- fork ではないこと

reviewer は merge しない。`roles.toml` の `merge` は全ロールで `false`。

## 秘密

- 秘密（App private key、AI API キー、`.env`）は **repo に置かない**
- 主: 別 Windows ユーザー `orcaworker` の profile に置き、ACL をそのユーザーだけにする
- 追加の防御: サービスアカウント + localhost ブローカー（B）を重ねられる
- オーナーの `gh` keyring は使わない（`orcaworker` から読めない）
- `.gitignore` と `scripts/ci/test_no_tracked_secrets.py` で混入を防ぐ
- App の作り方は `automation/apps.md`、鍵と token の扱いは `automation/KEYS.md`
- 3 人目以降のロールは `automation/roles.toml` に追加する。コードの変更は不要にする

## 監視

- dashboard: Issue / worktree / PR / review / merge / 失敗
- Orca: 各セッションのチャットとターミナル
- ログ: ホストと dashboard
- **AI は監視しない**。監視は GitHub の状態とローカルの状態を読むだけのコード
- coordinator はオーナーの疑問に答えるが、監視の判断はコードが行う

## GitHub の設定

- CI ruleset: `Gate` を required check にする（M2）。CI に bypass は作らない
- Review ruleset: `reviews` team の承認。**reviewer App は bypass にしない**
- Protected ruleset: CODEOWNERS（`reviews`）の承認
- 人間（owner）は Review / Protected を bypass できる。CI は bypass しない
- reviewer App は `reviews` team に入れない
- worker / reviewer の App は `checks: read` / `commit statuses: read` を持つ
- `issues: write` は coordinator のみ。worker / reviewer には付けない
- `workflows: write` は付けない。`.github/workflows` の変更は人が行う
- `checks` / `commit statuses` の **Write は付けない**

## 資産の置き場

| 資産 | 場所 |
| --- | --- |
| 仕様 | `docs/specs/automation.md` |
| policy / roles / App 手順 / 鍵の手順 | `automation/` |
| reviewer のコード / dashboard | `reviewer/` |
| エージェント手順 | `.opencode/skills/build-issue/`、`.opencode/skills/review-pr/` |
| 秘密 | `orcaworker` の profile（repo 外） |
| 状態 / ログ | ホストのローカル（資産ではない） |

## マイルストーン

| # | 内容 | 完了条件 |
| --- | --- | --- |
| M1 | 仕様 + policy + roles + App 手順 + skill + 秘密混入テスト | PR が通る |
| M2 | 別ユーザー `orcaworker` + Orca pair + ブローカー + App 作成 + `Gate` required | worker / reviewer の token が別スコープで動く |
| M3 | worker + OpenCode + dispatch | テスト Issue から PR |
| M4 | reviewer + OpenCode + dashboard | 既存 PR を dry-run レビューし、merge / close で解放 |
| M5 | 端到端 | 通常 PR を人間がマージできる |

## リスク

- worker / reviewer をオーナーと同じユーザーで動かすと `gh auth token` で keyring が読める。`orcaworker` で動かす
- GitHub App token の 1 時間失効
- ラベルの取り残し・二重起動
- reviewer のコンテキスト肥大と API コスト
- `Gate` 必須化と flaky test の関係

## 参照

- `automation/policy.toml`
- `automation/roles.toml`
- `automation/apps.md`
- `automation/KEYS.md`
- `.opencode/skills/build-issue/SKILL.md`
- `.opencode/skills/review-pr/SKILL.md`

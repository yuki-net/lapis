# 自動開発フロー（automation）

## この文書の役割

Issue から実装・レビュー・マージまでを自動化するときの、**役割・境界・状態遷移・秘密の扱い**を決める正本。
実装（M2 以降）はこの文書と `automation/` の policy / roles に従う。

## 目的

- 人間は Issue を別の場所で作り、**「#N を作って」と明示したときだけ**開始する
- worker が実装して PR を作り、reviewer がレビューして、条件を満たせばマージする
- 人間は**例外だけ**見る（保護パス、外部 PR、NG、リトライ上限）
- 監視は dashboard / ログ / Orca。**AI に監視させない**
- 仕様・skill・policy は repo に残す。秘密は repo に置かない

## 非目標

- 全 Issue の自動 pickup（Issue ができただけでは動かさない）
- AI による監視・通知の判断
- 保護パスの AI マージ
- 作業ごとにコンテナや VM を作ること

## 役割

| 役割 | 実体 | できること | できないこと |
| --- | --- | --- | --- |
| **人間** | あなた | Issue 作成、`#N を作って`、保護パスの承認、外部 PR のマージ | — |
| **コーディネータ** | Orca bot（ホスト） | Issue の確認、`dispatch.py` の実行 | merge、保護パスの変更 |
| **worker** | `worker-env` の OpenCode | 実装、PR、CI 修正 | merge、reviewer の鍵の読み取り |
| **reviewer** | `reviewer-env` の OpenCode | PR のレビュー、通常 PR のマージ | 保護パスのマージ、worker の鍵の読み取り |

- worker と reviewer は **別 GitHub App・別 UNIX ユーザー・別鍵**
- reviewer は Review ruleset の Integration bypass のみ。Protected は bypass しない
- コーディネータは merge しない（dispatch はラベルと worktree 作成だけ）

## 実行環境（Linux VM + Orca pair）

### 第一候補

- **Hyper-V + Ubuntu 24.04 LTS**、1 VM
- VM 内に 2 UNIX ユーザー:
  - `lapis-worker`: worker App の鍵、OpenCode、repo / worktree、Cargo / Gradle キャッシュ
  - `lapis-reviewer`: reviewer App の鍵、OpenCode / Junie、dashboard
- 各ユーザーが `orca serve` を起動し、ホストの Orca に `environment add` で pair する
  - worker: `:6768`
  - reviewer: `:6769`
- ホストの admin token は VM に存在しない
- 鍵・環境変数・キャッシュは各ユーザーの home に永続化する

### WSL2 を使う場合

- **automount を無効**にする（`/mnt/c` から Windows の `gh` token を読ませない）
- worker に **sudo を渡さない**
- repo は WSL の ext4 側に置く
- それでも隔離が弱いので、Hyper-V を第一候補とする

### 2 VM に分ける場合

- worker と reviewer を別 VM にする。ネットワーク・リソース・blast radius を分けたいとき
- 最初は 1 VM + 2 ユーザー。役割が増えたら `roles.toml` の `environment` を分ける

### Orca の接続

```powershell
# ホスト
orca environment add --name worker-env --pairing-code "orca://pair?code=..."
orca environment add --name reviewer-env --pairing-code "orca://pair?code=..."
orca worktree create --environment worker-env --name issue-123
```

- pairing link は `orca serve` の起動ごとに変わる。起動スクリプトがログから拾って `environment add` する
- pairing の公開アドレスはホストから届くアドレスにする
- VM 内の作業は Orca のチャット / ターミナル / dashboard で見える

## フロー

```text
人間: Issue を作成
  ↓
人間: 「#N を作って」
  ↓
coordinator: automation/dispatch.py --issue N
  - Issue の確認（open、受け入れ条件）
  - ai-building ラベル、ローカルロック
  - orca worktree create --environment worker-env --agent opencode --prompt "<Issue>"
  ↓
worker: 実装 → PR → CI
  ↓
reviewer: PR の最新 HEAD SHA を検知
  - PR ごとに新しい OpenCode セッション
  - diff / CI / policy を確認
  - 条件を満たせば reviewer App でマージ
  ↓
人間: 例外だけ確認（保護パス、外部、NG、上限）
```

## 発火

- **Issue ができただけでは動かさない**。人間の「#N を作って」がトリガー
- `automation/dispatch.py --issue N` が決定的に行う:
  1. Issue が open で、受け入れ条件が書かれている
  2. `ai-building` が付いていない
  3. ローカルロック（`~/.config/lapis/locks/`）が取れる
  4. `ai-building` を付ける
  5. `orca worktree create --environment worker-env --name issue-N --agent opencode --prompt "<Issue>"` で worker を起動
- 二重起動・競合はラベルとロックで防ぐ
- コーディネータが人間の token を使う場合でも、dispatch はラベルと worktree 作成だけを行い、merge しない
- 異常終了に備え、`ai-building` は heartbeat と `stale_building_minutes` で回収する

## worker

- worker App の installation token を使う。**ホストの token は使わない**
- token は wrapper が発行し、**残り 5 分で再発行**する
- OpenCode を基本、Junie CLI を fallback にする
- 作業後に PR を作る。PR の本文に Issue をリンクする
- 失敗の扱い:
  - 連続 2 回で `ai-needs-human`
  - 60 分で timeout
  - 保護パスを触った場合は自動マージ対象外
- worker は merge しない

## reviewer

- オープン PR を**決定的に** poll する（AI の判断ではない）
- 正本は **PR の最新 HEAD SHA**
  - 最新 SHA に対する reviewer の結果がなければキュー
  - 結果があればスキップ
- **PR ごとに新しい OpenCode セッション**。前の PR の文脈を持ち越さない
- 並列は 2〜4。PR ごとに claim して二重レビューを防ぐ
- push されたら古い判定を捨て、新しい SHA で再レビュー
- コメント・マージは冪等（同じ SHA で二重にしない）
- reviewer 自身の PR はレビューしない（別の reviewer か人間）

## 自動マージ

`automation/policy.toml` を正本とする。すべて満たすときだけ reviewer がマージする。

- CI の `Gate` が green
- reviewer の verdict が approve
- 保護パスを触っていない
- fork ではない
- 許可 author（worker App / 人間）である
- 差分が上限以内

満たさない場合は `ai-needs-human` を付けて人間に渡す。

## 秘密

- 秘密（App private key、AI API キー、`.env`）は **repo に置かない**
- VM の各ユーザーの home（`~/.config/lapis/<role>/`）に置く。パーミッションは 600
- `.gitignore` と `scripts/ci/test_no_tracked_secrets.py` で混入を防ぐ
- App の作り方は `automation/apps.md`、鍵と token の扱いは `automation/KEYS.md`
- 3 人目以降のロールは `automation/roles.toml` に追加する。コードの変更は不要にする

## 監視

- dashboard: Issue / worktree / PR / review / merge / 失敗
- Orca: 各 environment のチャットとターミナル
- ログ: VM と dashboard
- **AI は監視しない**。監視は GitHub の状態とローカルの状態を読むだけのコード

## GitHub の設定

- CI ruleset: M5 で `Gate` を required check にする
- Review ruleset: `reviews` team の承認。reviewer App は Integration bypass
- Protected ruleset: CODEOWNERS（`reviews`）の承認。reviewer App は bypass しない
- 人間（owner）は Review / Protected を bypass できる。CI は bypass しない
- worker App は `reviews` team に入れない
- App の `Checks` / `Commit statuses` は **Read のみ**（Write を付けると required check を偽造できる）

## 資産の置き場

| 資産 | 場所 |
| --- | --- |
| 仕様 | `docs/specs/automation.md` |
| policy / roles / App 手順 / 鍵の手順 | `automation/` |
| reviewer のコード / dashboard / Docker | `reviewer/` |
| エージェント手順 | `.opencode/skills/build-issue/`、`.opencode/skills/review-pr/` |
| 秘密 | VM の home（repo 外） |
| 状態 / ログ | VM のローカル（資産ではない） |

## マイルストーン

| # | 内容 | 完了条件 |
| --- | --- | --- |
| M1 | 仕様 + policy + roles + App 手順 + skill + 秘密混入テスト | PR が通る |
| M2 | Hyper-V Ubuntu + 2 ユーザー + Orca serve + pair | ホストから 2 environment が見える |
| M3 | worker + OpenCode + dispatch | テスト Issue から PR |
| M4 | reviewer + OpenCode + dashboard | 既存 PR を dry-run レビュー |
| M5 | App 作成 + reviewer bypass + Gate required | 通常 PR が自動マージ |
| M6 | Junie fallback、ローテーション、通知、lint ループ | 1 週間無人 |

## リスク

- Linux 版 Orca の入手と VM への導入
- pairing link の再発行と、ホストから VM への到達性
- GitHub App token の 1 時間失効
- ラベルの取り残し・二重起動
- reviewer のコンテキスト肥大と API コスト
- VM のディスク・CPU・Cargo キャッシュ
- `Gate` 必須化と flaky test の関係

## 参照

- `automation/policy.toml`
- `automation/roles.toml`
- `automation/apps.md`
- `automation/KEYS.md`
- `.opencode/skills/build-issue/SKILL.md`
- `.opencode/skills/review-pr/SKILL.md`

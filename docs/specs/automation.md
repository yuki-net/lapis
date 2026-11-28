# 自動開発フロー（automation）

## この文書の役割

Issue から実装・レビュー・マージまでを自動化するときの、**役割・境界・状態遷移・秘密の扱い**を決める正本。
実行環境は**この Windows ホスト**で、VM / GitHub App / `orcaworker` の分離は使わない。
実装（M2 以降）はこの文書と `automation/` の policy / roles に従う。

## 目的

- 人間は Issue を別の場所で作り、**「#N を作って」と明示したときだけ**開始する
- worker が実装・CI・レビュー指摘の反映を行い、PR を更新する
- **verifier** がマージ判断（PASS / FAIL）を出し、**skeptic / architect / minimalist** が役割別にレビューする
- verifier が PASS を出したら、**coordinator が owner 名義で merge** する
- 人間は**例外だけ**見る（保護パス、`.github/workflows`、外部 PR、3 回 FAIL、判断不能）
- 監視は dashboard / ログ / Orca。**AI に監視させない**
- 仕様・skill・policy は repo に残す。秘密は repo に置かない

## 非目標

- 全 Issue の自動 pickup（Issue ができただけでは動かさない）
- モデル違い（cross-model）レビュー
- AI による監視・通知の判断
- 保護パス / `.github/workflows` の AI 変更
- VM / コンテナを作ること
- GitHub App / installation token / `orcaworker` の分離

## 認証と境界（重要）

- コミット・PR・merge は **オーナーアカウント名義**で行う。worker と coordinator はオーナーの `gh` / git 認証を使う
- GitHub App、installation token、`orcaworker` ユーザー、ブローカーは**不採用**
- **同一アカウント運用なので、次の制約は GitHub 側では強制されない**
  - worker / reviewer は merge しない
  - reviewer は GitHub に書き込まない（コメント・review・label）
  - 保護パス / `.github/workflows` は AI が触らない
- これらは**プロンプトと手順の制約**であり、GitHub の権限・ruleset の強制ではない。owner の bypass 権限は存在する
- 保護パス / `.github/workflows` は従来どおり人が確認してマージする

## 役割

| 役割 | 担当 | 書き込み | merge |
| --- | --- | --- | --- |
| **人間** | Issue 作成、`#N を作って`、保護パス / workflows の承認、エスカレーション対応 | 可 | 可 |
| **coordinator** | Lapis マネージャ。監視、オーナーの疑問解決、`dispatch.py` の起動、verifier PASS 後の merge | owner 名義 | verifier PASS のときだけ |
| **worker** | LapisBot 型。実装、CI を通す、レビュー指摘の反映と push | owner 名義 | しない |
| **verifier** | マージ判断。head SHA でゲート再実行、実物検証、差分監査。PASS / FAIL | しない | しない |
| **skeptic** | 正しさ・網羅性。壊れる入力、未処理のエラー、race、未証明の前提 | しない | しない |
| **architect** | 構造。境界違反、結合点、暗黙の前提 | しない | しない |
| **minimalist** | 必要性。消せるもの、早すぎる抽象化、設定の乱立 | しない | しない |

- 全レビュワーは**同じモデル**でよい（cross-model は使わない）。役割とプロンプトで分ける
- coordinator は merge 以外の実装・レビューをしない
- reviewer は GitHub に書き込まない。所見は Orca / dashboard に返す

## レビュー構成（変更規模で決める）

変更行数とファイル数の**大きい方の区分**を使う。

| 区分 | 行数 | ファイル | レビュワー |
| --- | --- | --- | --- |
| 小 | < 50 | 1–2 | verifier + skeptic |
| 中 | 50–199 | 3–4 | verifier + skeptic + architect |
| 大 | 200+ | 5+ | verifier + skeptic + architect + minimalist |

行数・ファイル数のどちらかが大きい区分に達したら、その区分を使う（200 行 / 5 ファイルは大）。

- verifier は常に入る
- skeptic / architect / minimalist は **PASS / FAIL ではなく指摘**を返す
- verifier はレンズの指摘を確認したうえで **PASS / FAIL** を出す
- レビュワーは **PR ごとに新しいセッション**を開く。同じ PR の open 中は保持し、merge / close で解放する

## 実行環境（この Windows ホスト）

- Hyper-V / WSL2 の VM は作らない
- Orca の worktree と OpenCode セッションを、オーナーと同じ Windows アカウントで動かす
- worker: `orca worktree create --name issue-N --agent opencode --prompt "<Issue>"`
- reviewer: PR ごと・役割ごとに新しいセッション。**PR が open の間は保持し、merge / close で解放**する

## フロー

```text
人間: Issue を作成
  ↓
人間: 「#N を作って」
  ↓
coordinator: automation/dispatch.py --issue N
  - Issue の確認（open、受け入れ条件）
  - ai-building ラベル、ローカルロック
  - orca worktree create --name issue-N --agent opencode --prompt "<Issue>"
  ↓
worker: 実装 → CI → PR
  ↓
verifier + skeptic（+ architect + minimalist）: PR ごとに新しいセッション
  - verifier: head SHA のゲート、実物検証、差分監査、レンズ所見の確認
  - skeptic / architect / minimalist: 役割別の指摘
  ↓
verifier PASS → coordinator が owner 名義で merge
verifier FAIL → worker に差し戻し（3 回 FAIL でオーナーへ）
  ↓
merge / close でレビュワーセッションを解放
```

coordinator の dispatch は起動だけを行う。実装・レビューはしない。
coordinator の merge は verifier PASS のときだけ行う。

## 発火

- **Issue ができただけでは動かさない**。人間の「#N を作って」がトリガー
- `automation/dispatch.py --issue N` が決定的に行う:
  1. Issue が open で、受け入れ条件が書かれている
  2. `ai-building` が付いていない
  3. ローカルロック（`%LOCALAPPDATA%\lapis\locks\`）が取れる
  4. `ai-building` を付ける
  5. `orca worktree create --name issue-N --agent opencode --prompt "<Issue>"` で worker を起動
- 二重起動・競合はラベルとロックで防ぐ
- 異常終了に備え、`ai-building` は heartbeat と `stale_building_minutes` で回収する

## worker

- オーナーの `gh` / git 認証を使う。**GitHub App / ブローカーは使わない**
- 実装し、CI を通し、PR を作る。PR の本文に Issue をリンクする
- verifier / レンズの指摘を反映して push する
- 失敗の扱い:
  - 同じ PR で 3 回 FAIL したら停止し、coordinator がオーナーへエスカレーションする
  - 60 分で timeout
  - 保護パス / `.github/workflows` を触った場合は、自動フローを止めて人に渡す
- worker は merge しない（手順の制約）

## レビュー

### verifier（マージ判断）

- PR の**最新 HEAD SHA** を正本にする
- 最新 SHA の `Gate` を確認する
- head SHA でゲートを**再実行**する。GitHub の rerun ではなく、ローカルで同じ検証を回す
- **実物検証**: 差分だけでなく、ビルド・テスト・生成物を実際に確認する
- **差分監査**: 意図しない変更、保護パス、`.github/workflows`、生成物の混入を確認する
- skeptic / architect / minimalist の指摘を確認する
- **PASS / FAIL** を出す。保護パス / workflows / 外部 fork / 判断不能は PASS にしない
- GitHub には書き込まない（コメント・review・label をしない）

### skeptic（正しさ・網羅性）

- 壊れる入力、境界値、未処理のエラー
- race、順序、再試行、冪等性
- 未証明の前提、テストで確認されていない主張

### architect（構造）

- 境界違反、依存方向、状態の所有
- 結合点、重複、暗黙の前提
- 既存の設計との整合

### minimalist（必要性）

- 消せるもの、早すぎる抽象化
- 設定・分岐・互換層の乱立
- 変更に対して過剰な機構

## マージ

- worker もレビュワーも merge しない
- verifier が PASS を出したら、**coordinator が owner 名義で merge** する
- coordinator は次をすべて確認する:
  - 最新 HEAD SHA に対する verifier の PASS
  - 最新 HEAD SHA の `Gate` が success
  - 保護パス / `.github/workflows` を触っていない
  - 外部 fork ではない
- 1 つでも満たさない場合は merge せず、オーナーへエスカレーションする
- **同一アカウントなので、この禁止は GitHub では強制されない。手順とプロンプトで守る**

## 失敗・エスカレーション

- verifier FAIL → worker に差し戻す
- 同じ PR で **3 回 FAIL** → 停止し、オーナーへエスカレーション
- 保護パス / workflows / 外部 fork / 判断不能 → オーナーへ
- 60 分 timeout / stale → `ai-needs-human` を付けて止める

## 秘密

- 秘密（`gh` の認証、AI API キー、`.env`）は **repo に置かない**
- コミット・PR・merge は owner 名義なので、`gh` keyring を worker / coordinator が使う
- AI キーはロールごとに分ける（可能な範囲で）
- token をプロンプト・ログ・成果物に出さない
- `.gitignore` と `scripts/ci/test_no_tracked_secrets.py` で混入を防ぐ

## 監視

- dashboard: Issue / worktree / PR / review / merge / 失敗 / FAIL 回数
- Orca: 各セッションのチャットとターミナル
- ログ: ホストと dashboard
- **AI は監視しない**。監視は GitHub の状態とローカルの状態を読むだけのコード
- coordinator はオーナーの疑問に答えるが、監視の判断はコードが行う

## GitHub の設定

- CI ruleset: `Gate` を required check にする（bypass なし）
- Review ruleset: `reviews` team の承認。owner は bypass できる
- Protected ruleset: CODEOWNERS（`reviews`）の承認。owner は bypass できる
- **同一アカウント運用なので、ruleset で AI の merge を止められない**。verifier + coordinator の手順で止める
- 保護パス / `.github/workflows` は人が確認してマージする

## 資産の置き場

| 資産 | 場所 |
| --- | --- |
| 仕様 | `docs/specs/automation.md` |
| policy / roles / 認証と秘密 | `automation/` |
| reviewer のコード / dashboard | `reviewer/` |
| エージェント手順 | `.opencode/skills/build-issue/`、`.opencode/skills/review-pr/` |
| 秘密 | ホストの owner 環境（repo 外） |
| 状態 / ログ | ホストのローカル（資産ではない） |

## マイルストーン

| # | 内容 | 完了条件 |
| --- | --- | --- |
| M1 | 仕様 + policy + roles + skill + 秘密混入テスト | PR が通る |
| M2 | 役割別レビュー + verifier + coordinator merge + セッション管理 | 既存 PR を dry-run レビューし、PASS / FAIL を出せる |
| M3 | worker ループ + dispatch | テスト Issue から PR |
| M4 | dashboard + FAIL 回数 / エスカレーション | 3 回 FAIL で停止できる |
| M5 | 端到端 | 通常 PR を verifier PASS → coordinator merge できる |

## リスク

- 同一アカウントなので、merge 禁止 / read-only は GitHub で強制されない（手順とプロンプト依存）
- verifier が 1 人なので、判断の見落としがそのまま merge につながる
- 3 回 FAIL のカウントとセッション解放の取りこぼし
- `Gate` 必須化と flaky test の関係
- reviewer のコンテキスト肥大と API コスト
- 保護パス / workflows を AI が触る事故（手順で禁止）

## 参照

- `automation/policy.toml`
- `automation/roles.toml`
- `automation/KEYS.md`
- `.opencode/skills/build-issue/SKILL.md`
- `.opencode/skills/review-pr/SKILL.md`

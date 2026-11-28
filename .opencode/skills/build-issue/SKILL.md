---
name: build-issue
description: 人間が明示した Issue 番号を worker に渡し、実装から PR まで進める。全 Issue の自動 pickup はしない。
---

# Issue を worker に渡す

## 前提

- 人間が Issue を別の場所で作っている
- 人間が「#N を作って」と明示したときだけ実行する
- Issue には受け入れ条件が書かれている
- worker はこの Windows ホストの Orca worktree で動く（VM は使わない）
- worker はオーナー名義で commit / push / PR する（GitHub App は使わない）

## 手順

1. Issue を確認する

```sh
gh issue view N --json number,title,body,state,labels
```

- `state` が open
- 受け入れ条件が本文にある
- `ai-building` が付いていない

2. dispatch を実行する（M3 で実装）

```sh
python automation/dispatch.py --issue N
```

`dispatch.py` が決定的に行うこと:

- `ai-building` ラベル
- ローカルロック（`%LOCALAPPDATA%\lapis\locks\`）
- `orca worktree create --name issue-N --agent opencode --prompt "<Issue>"`
- worker セッションを起動する

3. worker の状態を確認する

- Orca の worker セッション
- PR の有無
- CI の状態
- verifier / skeptic / architect / minimalist の指摘

4. worker が行うこと

- 実装し、CI を通す
- PR を作り、本文に Issue をリンクする
- verifier / レンズの指摘を反映して push する
- merge はしない（手順の制約）

5. 停止条件

- 同じ PR で 3 回 FAIL
- 60 分を超えた
- 保護パス / `.github/workflows` を触った
- コンフリクトや仕様の矛盾

この場合は `ai-needs-human` を付け、理由を Issue / PR に残して止まる。
3 回 FAIL は coordinator がオーナーへエスカレーションする。

## 禁止

- 人間の明示なしに Issue を拾う
- 保護パス（`automation/`、`reviewer/`、`policy.toml` など）や `.github/workflows` を worker に編集させる
- worker に merge させる
- 秘密を repo やプロンプトに書く

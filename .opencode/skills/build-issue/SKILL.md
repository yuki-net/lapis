---
name: build-issue
description: 人間が明示した Issue 番号を worker に渡し、実装から PR まで進める。全 Issue の自動 pickup はしない。
---

# Issue を worker に渡す

## 前提

- 人間が Issue を別の場所で作っている
- 人間が「#N を作って」と明示したときだけ実行する
- Issue には受け入れ条件が書かれている

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
- ローカルロック（`~/.config/lapis/locks/`）
- `orca worktree create --environment worker-env --name issue-N --agent opencode --prompt "<Issue>"`
- worker に Issue を渡す

3. worker の状態を確認する

- Orca の `worker-env` のチャット
- PR の有無
- CI の状態

4. 停止条件

- worker の連続失敗が 2 回
- 60 分を超えた
- 保護パスを触った
- コンフリクトや仕様の矛盾

この場合は `ai-needs-human` を付け、理由を Issue / PR に残して止まる。

## 禁止

- 人間の明示なしに Issue を拾う
- 保護パス（`automation/`、`reviewer/`、`policy.toml` など）を worker に編集させる
- worker に merge させる
- 秘密を repo やプロンプトに書く

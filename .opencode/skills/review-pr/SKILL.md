---
name: review-pr
description: PR を PR ごとの新しいセッションでレビューし、所見をローカルに返す。GitHub への書き込みと merge はしない。
---

# PR をレビューする

## 前提

- reviewer App の **read-only** token を使う（`contents: read` / `pull requests: read` / `checks: read` / `commit statuses: read`）
- オーナーの token / keyring は使わない
- **GitHub に書き込まない**（コメント・ラベル・review・merge をしない）
- PR ごとに新しいセッションを開く（前の PR の文脈を持ち越さない）
- セッションは PR が open の間は保持し、merge / close で解放する

## 手順

1. 対象 PR の **最新 HEAD SHA** を確認する

```sh
gh pr view N --json number,headRefOid,headRepository,isCrossRepository,author,files
```

2. 最新 SHA に対する結果が既にローカルにあればスキップする
   - ローカル状態（SHA キー）を正本にする
   - GitHub には書かないので、check run やマーカーコメントは使わない

3. PR を claim する（1 PR = 1 レビュー、同時 2〜4）

4. 新しい OpenCode セッションで、次を渡してレビューする

- PR の diff（API で取得。コードは実行しない）
- 最新 SHA の CI 結果（`checks: read` / `commit statuses: read` で読む）
- `automation/policy.toml`
- Issue / PR の本文

5. verdict をローカルに返す

- `approve`: policy を満たす
- `needs-human`: 保護パス、外部 fork、NG、上限超過、判断できない
- `request-changes`: worker が直せる具体的な指摘

verdict は dashboard / Orca に残す。GitHub のコメントやラベルは付けない。

6. セッションの解放

merge / close を検知したら、その PR のセッションを閉じる。
open の間は保持し、push されたら新しい SHA として再レビューする。

## 並列

- PR ごとに 1 セッション
- 同時 2〜4
- claim できなかった PR は待つ
- 同じ PR を二重にレビューしない

## 禁止

- GitHub への書き込み（コメント・ラベル・review・merge）
- 保護パスの変更
- 外部 fork の merge
- worker の鍵・reviewer の鍵を混ぜる
- オーナーの `gh` token / keyring を使う
- 監視を AI に任せる（監視は dashboard / GitHub の状態）

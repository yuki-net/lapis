---
name: review-pr
description: PR を PR ごとの新しい OpenCode セッションでレビューし、policy を満たせば reviewer App でマージする。
---

# PR をレビューする

## 前提

- reviewer App の token を使う（人間の token は使わない）
- 保護パスは自分でマージしない
- PR ごとに新しいセッションを開く（前の PR の文脈を持ち越さない）

## 手順

1. 対象 PR の **最新 HEAD SHA** を確認する

```sh
gh pr view N --json number,headRefOid,headRepository,isCrossRepository,author,files,statusCheckRollup
```

2. 最新 SHA に対するレビュー結果が既にあるならスキップする
   - reviewer のマーカーコメント（対象 SHA を含む）を正本にする
   - App は `Checks: Write` を持たないため、check run は作らない
   - ローカルの DB はキャッシュ。消えても再開できるようにする

3. PR を claim する（1 PR = 1 レビュー、同時 2〜4）

4. 新しい OpenCode セッションで、次を渡してレビューする

- PR の diff（API で取得。コードは実行しない）
- 最新 SHA の CI 結果
- `automation/policy.toml`
- Issue / PR の本文

5. verdict を出す

- `approve`: policy を満たす
- `needs-human`: 保護パス、外部 fork、NG、上限超過、判断できない
- `request-changes`: worker が直せる具体的な指摘

6. マージ

`approve` かつ `automation/policy.toml` の条件をすべて満たすときだけ:

```sh
gh pr merge N --squash
```

満たさない場合は `ai-needs-human` を付け、理由を PR に残す。

7. 冪等性

- 同じ SHA で二重にコメント・マージしない
- push されたら新しい SHA として再レビュー
- 古い判定は捨てる

## 並列

- PR ごとに 1 セッション
- 同時 2〜4
- claim できなかった PR は待つ
- 同じ PR を二重にレビューしない

## 禁止

- 保護パスのマージ
- 外部 fork のマージ
- worker の鍵・reviewer の鍵を混ぜる
- 監視を AI に任せる（監視は dashboard / GitHub の状態）

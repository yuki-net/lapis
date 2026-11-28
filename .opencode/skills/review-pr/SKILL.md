---
name: review-pr
description: PR を PR ごと・役割ごとの新しいセッションでレビューする。verifier が PASS/FAIL を出し、skeptic / architect / minimalist が指摘を返す。レビュワーは GitHub に書き込まず、merge もしない。
---

# PR をレビューする

## 前提

- レビュワーは **PR ごと・役割ごとに新しいセッション**を開く（前の PR の文脈を持ち越さない）
- 同じ PR の open 中はセッションを保持し、merge / close で解放する
- レビュワーは **GitHub に書き込まない**（コメント・review・label）。所見は Orca / dashboard に返す
- レビュワーは **merge しない**
- コミット・PR・merge は owner 名義。同一アカウントなので、この禁止は GitHub の権限ではない。**手順の制約**として守る
- モデルは全員同じでよい。役割とプロンプトで分ける

## レビュー構成

変更行数とファイル数の大きい方の区分を使う。

| 区分 | 行数 | ファイル | レビュワー |
| --- | --- | --- | --- |
| 小 | < 50 | 1–2 | verifier + skeptic |
| 中 | 50–199 | 3–4 | verifier + skeptic + architect |
| 大 | 200+ | 5+ | verifier + skeptic + architect + minimalist |

行数・ファイル数のどちらかが大きい区分に達したら、その区分を使う（200 行 / 5 ファイルは大）。

## 手順

1. 対象 PR の **最新 HEAD SHA** を確認する

```sh
gh pr view N --json number,headRefOid,headRepository,isCrossRepository,author,files,additions,deletions,statusCheckRollup
```

2. 最新 SHA に対する結果が既にローカルにあればスキップする
   - ローカル状態（SHA キー）を正本にする
   - GitHub には書かない

3. PR を claim する（1 PR = 1 レビュー。役割ごとにセッションを分ける）

4. レビュワーを起動する

- verifier: 常に
- skeptic: 常に
- architect: 中・大
- minimalist: 大

## verifier（マージ判断）

- 最新 HEAD SHA の `Gate` を確認する
- head SHA でゲートを**再実行**する。GitHub の rerun は coordinator に依頼し、verifier はローカルで同じ検証を回す
- **実物検証**: ビルド・テスト・生成物を実際に確認する
- **差分監査**: 意図しない変更、保護パス、`.github/workflows`、生成物の混入を確認する
- skeptic / architect / minimalist の指摘を確認する
- **PASS / FAIL** を出す
- 保護パス / workflows / 外部 fork / 判断不能は **PASS にしない**

## skeptic（正しさ・網羅性）

- 壊れる入力、境界値、未処理のエラー
- race、順序、再試行、冪等性
- 未証明の前提、テストで確認されていない主張
- PASS / FAIL は出さず、指摘を返す

## architect（構造）

- 境界違反、依存方向、状態の所有
- 結合点、重複、暗黙の前提
- 既存の設計との整合
- PASS / FAIL は出さず、指摘を返す

## minimalist（必要性）

- 消せるもの、早すぎる抽象化
- 設定・分岐・互換層の乱立
- 変更に対して過剰な機構
- PASS / FAIL は出さず、指摘を返す

## マージ

- verifier PASS → coordinator が owner 名義で merge する
- verifier FAIL → worker に差し戻す
- 同じ PR で 3 回 FAIL → 停止し、オーナーへエスカレーション
- 保護パス / workflows / 外部 fork → オーナーへ

レビュワーは merge しない。coordinator も保護パス / workflows は merge しない。

## 禁止

- GitHub への書き込み（コメント・review・label・merge）
- 保護パス / `.github/workflows` の変更
- worker / reviewer の秘密を混ぜる
- 監視を AI に任せる（監視は dashboard / GitHub の状態）

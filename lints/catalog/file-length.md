---
id: file-length
remedy: design
scope: rust
implementation: scripts.check_architecture:check_file_length
test: scripts.ci.test_catalog:CatalogTests.test_file_length_fixtures
added: 2026-11-23
---
# ファイル行数の上限（3000 行）

## 何を検知するか

3000 行を超える Rust ファイル。`target/`・`vendor/`・`node_modules/` は除く。

- 上限を超える**新しい**ファイル → エラー（baseline への記録は人間の判断）
- `lints/baseline.json` より**増えた** → エラー
- baseline より**減った** → baseline を下げるまでエラー（ラチェット）
- 上限以下になったら baseline から削除する

## 検知ロジックの場所

`scripts/check_architecture.py` の `check_file_length()`。既存違反は
`lints/baseline.json` に記録する。

## エラーメッセージ

```
clients/state/src/lib.rs: 3877 行 -> 4000 行に増えた（上限 3000 行）
```

## 直し方

1. まずファイルを分割する（test module・型・impl を別ファイルへ）。これが本来の修正
2. どうしても今の PR で分割できない場合だけ、人間が `lints/baseline.json` の値を
   **実際の行数**に更新する。増やす更新はレビューで理由を確認する

## 例外

なし。`lints/baseline.json` に記録された 4 ファイルだけが現状の負債で、
これは「増やさない」ためのラチェットであり、恒久的な免除ではない。
対象ファイルの分割そのものは別 Issue で行う。

## 導入背景

大きなファイルは、変更のたびにコンフリクトとレビュー負担を増やし、エージェントが
全体を読み切れなくなる。`AGENTS.md` の「不要な抽象化を作らない」と衝突しないよう、
**まず増加を止める**ところから始める。既存の 4 ファイル（#346 で確認）を baseline
として記録し、触った PR で自然に分割が進むようにする。

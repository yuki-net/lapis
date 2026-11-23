---
id: protected-paths
remedy: policy
why_not_design: lint 基盤を育てるには lints/ と CI 定義の変更が必要で、設計で禁止できない。変更の妥当性は人間しか判断できないため、変更自体を人間の確認に回す。
scope: repository
implementation: scripts.ci.policy:PROTECTED
test: scripts.ci.test_catalog:CatalogTests.test_protected_paths_fixtures
added: 2026-11-23
---
# 保護パス（lint 基盤・CI 定義）の変更

## 何を検知するか

PR の変更 path が次のいずれかに一致したら、`Policy` チェックを**無条件で失敗**
させる。ラベル・コメントによる免除はない（AI と人間が同じアカウントで動くため、
AI が自分で免除できてしまう）。

| 規則 | 対象 |
| --- | --- |
| `lints/*` | ルールのカタログ・baseline・fixture |
| `AGENTS.md` | エージェント向けの規約 |
| `.github/*` | ワークフロー・アクション |
| `scripts/check*.py` | アーキテクチャ検査・CI 入口 |
| `scripts/architecture-exceptions.json` | 依存方向の例外 |
| `scripts/ci/*` | CI の定義・実行・routing |
| `clippy.toml` / `rust-toolchain.toml` / `rustfmt.toml` | 静的検査と toolchain |

## 検知ロジックの場所

`scripts/ci/policy.py`。CI では `.github/workflows/ci.yml` の `policy` job が
`pull_request` のときだけ走る（`develop` / `main` への push では走らせない。
マージ済みの正当な変更で恒久的に赤くしないため）。

## エラーメッセージ

```
::error file=lints/README.md::保護パス lints/README.md が変更された（規則: lints/*）
```

## 直し方

1. 差分を人間が確認する（何のためにルール・CI を変えるのか、抜け道を作っていないか）
2. 問題なければ、管理者権限で必須チェックを迂回してマージする
3. 将来マージ専用エージェントを入れる場合も、このチェックを迂回できる権限は与えない

## 例外

なし。ただし次の 2 つは運用上の例外として扱う。

- **bootstrap**: この仕組みを導入する PR 自体が保護パスを触るため赤くなる。
  導入 PR は管理者がマージし、赤を消すための特別扱いは追加しない
- **ルールの追加**: カタログ・fixture・実装を同じ PR で足す。赤くなるのは意図どおり

## 導入背景

`AGENTS.md` と `docs/specs/` に書いた規約は、エージェントが読まなければ効かない。
一度直した指摘を lint／テストに落とし、**その lint 自体を AI が黙って弱められない**
ようにするための最後の砦がこのチェック（#346）。

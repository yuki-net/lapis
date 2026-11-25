# Contributing

## 開発の流れ

1. Issue を立てる（不具合・提案・質問）
2. ブランチを切る
   - write 権限がある人: このリポジトリに branch を作る
   - 外部の人: リポジトリを fork して、fork に branch を作る
3. PR を出す
4. CI を通す
5. レビューを受けてマージする

## ローカルでの検証

CI と同じ検証をローカルで回せます。

```sh
python -m scripts.ci.plan --selected --reason   # 変更から route を選ぶ
python -m scripts.ci.watch --worktree           # 選ばれた route を並列実行
```

`cargo fmt` と `cargo clippy` は CI の `Format` / `Lint` と同じ設定です。

## PR

- `.github/pull_request_template.md` に従う
- CI を必ず通す
- **保護パス**（`CODEOWNERS` に記載）を変更する場合は、maintainer のレビューが必要
  - `lints/`、`AGENTS.md`、`.github/`、`scripts/ci/`、`scripts/check*.py`、`Cargo.toml`、`Cargo.lock`、`protocol/`、`release/` など
- AI を使って変更した場合は、PR にモデル名を書く

## ライセンス

このリポジトリは **MIT** です。PR の貢献は MIT ライセンスで提供されたものとみなします。

## セキュリティ

脆弱性は公開 Issue ではなく [SECURITY.md](SECURITY.md) の手順で報告してください。

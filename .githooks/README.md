# Git hooks

このリポジトリは `.githooks` をGit hooksの保存先として使用します。

初回だけ、リポジトリルートで次を実行してください。

```sh
git config core.hooksPath .githooks
```

- `pre-commit`: Rustのfmtとworkspace全体の`cargo check --locked --workspace --all-targets`。変更関連のテストはコミット前に手元で実行します。
- 全検証はGitHub CIで実行します。ローカルで同じ検証を行う場合は
  `python scripts/check.py backend` / `python scripts/check.py desktop` を使用します。
- `pre-push`による全検証の重複はありません。GitHub App経由の更新でもCIは同じ検証を実行します。

フックはGit for WindowsのGit Bash、macOS、Linuxで実行できるPOSIX shell形式です。

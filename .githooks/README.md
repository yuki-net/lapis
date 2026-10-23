# Git hooks

このリポジトリは `.githooks` をGit hooksの保存先として使用します。

初回だけ、リポジトリルートで次を実行してください。

```sh
git config core.hooksPath .githooks
```

- `pre-commit`: Rustのfmtのみ。変更関連のテストはコミット前に手元で実行します。
- `pre-push`: workspace check / test / clippy
- 全検証はGitHub CIで実行します。

フックはGit for WindowsのGit Bash、macOS、Linuxで実行できるPOSIX shell形式です。

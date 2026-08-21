# Git hooks

このリポジトリは `.githooks` をGit hooksの保存先として使用します。

初回だけ、リポジトリルートで次を実行してください。

```sh
git config core.hooksPath .githooks
```

- `pre-commit`: fmtとworkspace check
- `pre-push`: fmt、workspace test、clippy、workspace build

フックはGit for WindowsのGit Bash、macOS、Linuxで実行できるPOSIX shell形式です。

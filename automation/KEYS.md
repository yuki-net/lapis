# 鍵と token の扱い

## 原則

- App private key、AI API キー、`.env` は **repo に置かない**
- VM の各ユーザーの home に置く（600）
- `scripts/ci/test_no_tracked_secrets.py` が追跡ファイルを検査する
- `.gitignore` は事故防止のバックストップ。保管場所ではない

## 置き場

```text
~/.config/lapis/worker/
  app.pem      # worker App の private key（600）
  env          # GITHUB_APP_ID / GITHUB_APP_INSTALLATION_ID / AI key（600）

~/.config/lapis/reviewer/
  app.pem
  env

~/.config/lapis/locks/    # dispatch のロック
```

- worker は reviewer の home を読めない（別 UNIX ユーザー）
- reviewer は worker の home を読めない
- ホストの Windows 側の `gh` token は VM に置かない

## installation token

GitHub App は private key → JWT（10 分）→ installation token（1 時間）で認証する。

- JWT: `iss=app_id`、`exp=now+9分` 程度
- installation token: `POST /app/installations/{id}/access_tokens`
- 1 時間で失効するので、**残り 5 分で再発行**する wrapper を使う
- `GH_TOKEN` を都度渡す。`GH_CONFIG_DIR` はロールごとに分ける
- worker は長いビルド中に切れないよう、`git` の credential helper / `GIT_ASKPASS` も wrapper 経由にする

## AI キー

- worker と reviewer で別のキー（同じ provider でも可）
- reviewer のキーは reviewer の home のみ
- Junie CLI のキーも同じくロールごとに分ける

## ローテーション

1. App の private key を新しく生成
2. VM の `app.pem` を差し替える（600）
3. 古い key を revoke する
4. 次の dispatch / review で新しい token になることを確認

## 禁止

- repo 内に `app.pem` / `.env` / AI キーを置く
- worker に reviewer の key を渡す
- worker に `Administration` / `Workflows` を付ける
- `Checks` / `Commit statuses` の **Write** を付ける（required check の偽造防止。Read は可）

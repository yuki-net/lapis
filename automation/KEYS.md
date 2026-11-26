# 鍵と token の扱い

## 原則

- App private key、AI API キー、`.env` は **repo に置かない**
- 鍵は**ブローカーのアカウントだけ**が読める場所に置く
- オーナーの `gh` ログイン（keyring、scopes: repo/workflow）は worker / reviewer に渡さない
- `scripts/ci/test_no_tracked_secrets.py` が追跡ファイルを検査する
- `.gitignore` は事故防止のバックストップ。保管場所ではない

## 境界（未決定）

同一 Windows ユーザーでは `gh auth token` でオーナーの keyring が読めるため、
`gh` の shim だけでは境界にならない。次のどちらかを選ぶ。

### A. 別 Windows ローカルユーザー

- `lapis-worker` / `lapis-reviewer` を別ユーザーとして作り、それぞれの Orca ランタイムで動かす
- 鍵は `C:\Users\lapis-worker\.lapis\secrets\` などに置き、ACL をそのユーザーだけにする
- オーナーは管理者なので読める。境界は**AI プロセスに対して**であり、オーナーに対してではない

### B. 専用サービスアカウント + localhost ブローカー（推奨）

- `lapis-broker` サービスアカウントでブローカーを常駐させる
- 鍵は `C:\ProgramData\lapis\secrets\` に置き、ACL を `lapis-broker` だけにする
- worker / reviewer は localhost のブローカーから**短命 token** を受け取る
- 鍵がエージェントのユーザーに存在しないため、A より漏えい面が小さい

どちらにするかは未決定。決めるまで鍵の実体は作らない。

## 置き場（B の場合）

```text
C:\ProgramData\lapis\secrets\
  worker\app.pem       # worker App の private key（ACL: lapis-broker）
  reviewer\app.pem     # reviewer App の private key
  worker\ai.env        # AI API キー
  reviewer\ai.env

%LOCALAPPDATA%\lapis\locks\   # dispatch のロック（秘密ではない）
```

## ブローカー

- App private key → JWT（10 分）→ installation token（1 時間）
- JWT: `iss=app_id`、`exp=now+9分` 程度
- installation token: `POST /app/installations/{id}/access_tokens`
- localhost だけで待ち受ける。worker / reviewer はロールを指定して token を受け取る
- **1 時間で失効するので、残り 5 分で再発行**する
- `gh` / `git` の子プロセスにだけ `GH_TOKEN` / `GIT_ASKPASS` を渡す
- token をプロセスの環境、ログ、ファイル、シェル履歴に残さない
- `GH_CONFIG_DIR` はロールごとに分ける（keyring は使わない）

## AI キー

- worker と reviewer で別のキー（同じ provider でも可）
- reviewer のキーは reviewer のブローカー領域だけに置く
- Junie CLI のキーも同じくロールごとに分ける

## ローテーション

1. App の private key を新しく生成
2. ブローカーの `app.pem` を差し替える（ACL を維持）
3. 古い key を revoke する
4. 次の dispatch / review で新しい token になることを確認

## 禁止

- repo 内に `app.pem` / `.env` / AI キーを置く
- worker に reviewer の key を渡す
- worker / reviewer にオーナーの `gh` token を渡す
- worker に `Administration` を付ける。`Workflows` は必要なときだけ付ける
- `Checks` / `Commit statuses` の **Write** を付ける（required check の偽造防止）

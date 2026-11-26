# 鍵と token の扱い

## 原則

- App private key、AI API キー、`.env` は **repo に置かない**
- 鍵は**AI プロセスとは別の Windows ユーザー**が読める場所に置く
- オーナーの `gh` ログイン（keyring、scopes: repo/workflow）は worker / reviewer に渡さない
- `scripts/ci/test_no_tracked_secrets.py` が追跡ファイルを検査する
- `.gitignore` は事故防止のバックストップ。保管場所ではない

## 境界（A を採用、B は追加の防御）

同一 Windows ユーザーでは `gh auth token` でオーナーの keyring が読めるため、
`gh` の shim だけでは境界にならない。worker / reviewer は**別の Windows ローカルユーザー
`orcaworker`** で動かす。

### A. 別 Windows ローカルユーザー `orcaworker`（採用）

- `orcaworker` で `orca serve` を起動し、オーナーの Orca に pair する
- 鍵は `C:\Users\orcaworker\.lapis\secrets\` に置き、ACL を `orcaworker` だけにする
- オーナーの keyring は `orcaworker` から見えない
- オーナーは管理者なので鍵を読める。境界は**AI プロセスに対して**であり、オーナーに対してではない

### B. 専用サービスアカウント + localhost ブローカー（追加の防御）

- A と排他ではない。鍵をさらに隠したいときに重ねる
- `lapis-broker` サービスアカウントでブローカーを常駐させ、鍵を
  `C:\ProgramData\lapis\secrets\` に置き、ACL を `lapis-broker` だけにする
- worker / reviewer は localhost のブローカーから**短命 token** を受け取る

## 置き場（A の場合）

```text
C:\Users\orcaworker\.lapis\secrets\
  worker\app.pem       # worker App の private key（ACL: orcaworker）
  reviewer\app.pem     # reviewer App の private key
  worker\ai.env        # AI API キー
  reviewer\ai.env

C:\Users\orcaworker\AppData\Local\lapis\locks\   # dispatch のロック（秘密ではない）
```

`automation/roles.toml` の `private_key` は **App 作成後に埋める**。それまでは空のまま。

## ブローカー

- App private key → JWT（10 分）→ installation token（1 時間）
- JWT: `iss=app_id`、`exp=now+9分` 程度
- installation token: `POST /app/installations/{id}/access_tokens`
- `orcaworker`（B を重ねる場合はサービスアカウント）で動き、localhost だけで待ち受ける
- worker / reviewer はロールを指定して token を受け取る
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
2. `orcaworker` の `app.pem` を差し替える（ACL を維持）
3. 古い key を revoke する
4. 次の dispatch / review で新しい token になることを確認

## 禁止

- repo 内に `app.pem` / `.env` / AI キーを置く
- worker に reviewer の key を渡す
- worker / reviewer にオーナーの `gh` token を渡す
- worker / reviewer に `Administration` を付ける
- worker / reviewer に `Workflows` を常用で付ける。`.github/workflows` の変更は人が行い、
  必要な PR だけ一時的に昇格する
- worker / reviewer に `Issues: Write` を付ける（ラベルは coordinator）
- `Checks` / `Commit statuses` の **Write** を付ける（required check の偽造防止）

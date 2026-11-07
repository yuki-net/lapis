# Web Bridge

ブラウザーのWebSocketを、同じPC上のHostが受け付けるQUIC接続へ転送する独立実行ファイルです。認証やWorkspace操作の判断はHostが行います。

```powershell
cargo run -p web-bridge -- --listen 127.0.0.1:8787 --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der --origin http://localhost:4747
```

`--host` と `--certificate` は起動中Hostのアドレスと公開証明書です。証明書とサーバー名を検証し、検証失敗時に接続を許可しません。Hostへの接続は常にloopbackに限定し、指定したOriginと `lapis.v0` subprotocolだけを受け付けます。Webページ自体は配信しません。

Hostを固定ポートで起動し、再起動時に `--replace-certificate` を指定すると、公開証明書を同じパスへ更新できます。Bridgeは新しい接続ごとに証明書を読み直すため、Bridge自体の再起動は不要です。証明書ファイルはHostとBridgeを実行するユーザーが管理し、他のユーザーが変更できない場所に置いてください。Host再起動後のClient認証は別途やり直す必要があります。

## 別PCから開く

**SSHポート転送（既定のまま）**

Webの開発サーバーとBridgeのTCPポートをSSHで転送し、ブラウザーから転送先のlocalhostを開きます。待ち受けはloopbackのままで、Originもlocalhostのまま使えます。

**Tailscaleで直接つなぐ**

Bridgeをtailnetへ公開する場合は、明示的に `--allow-remote` を付け、ブラウザーが使うOriginを指定します。

```powershell
cargo run -p web-bridge -- --listen 0.0.0.0:8787 --allow-remote --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der --origin http://100.64.0.1:4747
```

Webの開発サーバーもtailnetで待ち受けます。

```powershell
npm run dev -- --host 0.0.0.0 --port 4747
```

ページは `http://TAILSCALE_IP:4747/?url=ws://TAILSCALE_IP:8787/remote&workspace=WORKSPACE_ID&token=PAIRING_TOKEN` を開きます。

`--allow-remote` を付けると、tailnet内の端末からページとBridgeへ到達できます。Origin検証は認証ではないため、Hostへの接続に必要なcredentialやpairing tokenとは別に、Tailscale ACLで到達端末を制限してください。Windowsファイアウォールで該当ポートの受信許可も必要です。

## OIDC ログイン

次の環境変数が設定された場合、`GET /auth/oidc/start` と `GET /auth/oidc/callback` が有効になります。

- `LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT`（HTTPS）
- `LAPIS_WEB_OIDC_TOKEN_ENDPOINT`（HTTPS）
- `LAPIS_WEB_OIDC_CLIENT_ID`
- `LAPIS_WEB_OIDC_CLIENT_SECRET`（任意。設定時は HTTP Basic で送信）
- `LAPIS_WEB_OIDC_REDIRECT_URI`（HTTPS、`/auth/oidc/callback` を指定）
- `LAPIS_WEB_OIDC_SCOPES`（任意、既定 `openid profile`）

`/auth/oidc/start` は PKCE の verifier / state を生成して IdP へリダイレクトします。state は Bridge のメモリーに5分間保持し、callback で一度だけ消費します。`/auth/oidc/callback` は code を token endpoint で ID Token と交換し、opener へ `{ type: "lapis.oidc", idToken }` を `postMessage` してウィンドウを閉じます。Token は URL・ログ・Cookie に載せません。

Web 側はこの URL をポップアップで開き、`postMessage` の送信元が Bridge の Origin と一致することを確認してください。callback の失敗は 400（state/code 不正）または 502（token endpoint 失敗）になります。

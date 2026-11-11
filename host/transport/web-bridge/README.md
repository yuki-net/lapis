# Web Bridge

ブラウザーのWebSocketを、同じPC上のHostが受け付けるQUIC接続へ転送する独立実行ファイルです。認証やWorkspace操作の判断はHostが行います。

```powershell
cargo run -p web-bridge -- --connection-info target/dev-web/connection.json
```

待受とOriginの既定は `127.0.0.1:4757` と `http://127.0.0.1:4747` です。優先順はCLI引数、環境変数、既定値で、`LAPIS_WEB_BRIDGE_LISTEN` / `LAPIS_WEB_BRIDGE_ORIGIN` でも指定できます。開発時の起動手順は [開発時の起動](../../../docs/dev.md) を参照してください。

`--connection-info` は、Hostが `--connection-info` で書き出した接続情報ファイルのパスです。中身は待受アドレス・サーバー名・Workspace ID・公開証明書だけで、tokenや秘密鍵は含みません。Bridgeは起動時にファイルが読めるまで待ち（待機中は10秒ごとに状況を標準エラーへ出し、Ctrl+Cで中止できます）、読めたらブラウザーで開くURL（`http://127.0.0.1:4747/`）を標準エラーへ `open ...` の形で1行出します。接続先 URL・Workspace ID・token は URL に載せません。Workspace ID は別行に表示します。Hostより先にBridgeを起動して構いません。Hostの初回ビルドに時間がかかってもBridgeは終了しません。

接続先を明示する場合は `--host` / `--server-name` / `--certificate` を指定します。`--connection-info` との併用はエラーです。

```powershell
cargo run -p web-bridge -- --listen 127.0.0.1:4757 --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der --origin http://127.0.0.1:4747
```

`--host` と `--certificate` は起動中Hostのアドレスと公開証明書です。証明書とサーバー名を検証し、検証失敗時に接続を許可しません。Hostへの接続は常にloopbackに限定し、指定したOriginと `lapis.v0` subprotocolだけを受け付けます。Webページ自体は配信しません。

Hostを固定ポートで起動し、再起動時に `--replace-certificate` を指定すると、公開証明書を同じパスへ更新できます。Bridgeは新しい接続ごとに証明書を読み直すため、Bridge自体の再起動は不要です。`--connection-info` を使う場合も同じで、Hostが再起動してアドレスや証明書が変われば次の接続から追従します。接続情報を読めない場合はその接続だけ失敗し、Bridgeは停止しません。証明書ファイルはHostとBridgeを実行するユーザーが管理し、他のユーザーが変更できない場所に置いてください。Host再起動後のClient認証は別途やり直す必要があります。

## 別PCから開く

**SSHポート転送（既定のまま）**

Webの開発サーバーとBridgeのTCPポートをSSHで転送し、ブラウザーから転送先のlocalhostを開きます。待ち受けはloopbackのままで、Originもlocalhostのまま使えます。

**Tailscaleで直接つなぐ**

Bridgeをtailnetへ公開する場合は、明示的に `--allow-remote` を付けます。`--listen` を省くと `0.0.0.0:4757` で待ち受けます。

```powershell
cargo run -p web-bridge -- --allow-remote --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der
```

Webの開発サーバーもtailnetで待ち受けます。`--host` を付けると `0.0.0.0:4747` になります。

```powershell
cargo run -p lapis-web -- --dev --host --dist clients/web/dist
```

ページは `http://TAILSCALE_IP:4747/` を開き、一覧またはログインフォームから Host URL（`ws://TAILSCALE_IP:4757/remote`）・Workspace ID・pairing token を入力します。接続後の URL は `/<host>/<workspace>` になります。token をリンクで渡す場合は `#/invite?url=<bridge URL>&workspace=<ID>&token=<token>` の fragment 形式を使います。

bridge は Origin がリクエストの Host と同じホスト名なら許可します。別名で開く場合やプロキシを挟む場合だけ `--origin` を指定してください。

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

# Web Bridge

ブラウザーのWebSocketを、同じPC上のHostが受け付けるQUIC接続へ転送する独立実行ファイルです。認証やWorkspace操作の判断はHostが行います。

```powershell
cargo run -p web-bridge -- --listen 127.0.0.1:8787 --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der --origin http://localhost:5173
```

`--host` と `--certificate` は起動中Hostのアドレスと公開証明書です。証明書とサーバー名を検証し、検証失敗時に接続を許可しません。Hostへの接続は常にloopbackに限定し、指定したOriginと `lapis.v0` subprotocolだけを受け付けます。Webページ自体は配信しません。

Hostを固定ポートで起動し、再起動時に `--replace-certificate` を指定すると、公開証明書を同じパスへ更新できます。Bridgeは新しい接続ごとに証明書を読み直すため、Bridge自体の再起動は不要です。証明書ファイルはHostとBridgeを実行するユーザーが管理し、他のユーザーが変更できない場所に置いてください。Host再起動後のClient認証は別途やり直す必要があります。

## 別PCから開く

**SSHポート転送（既定のまま）**

Webの開発サーバーとBridgeのTCPポートをSSHで転送し、ブラウザーから転送先のlocalhostを開きます。待ち受けはloopbackのままで、Originもlocalhostのまま使えます。

**Tailscaleで直接つなぐ**

Bridgeをtailnetへ公開する場合は、明示的に `--allow-remote` を付け、ブラウザーが使うOriginを指定します。

```powershell
cargo run -p web-bridge -- --listen 0.0.0.0:8787 --allow-remote --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der --origin http://100.64.0.1:5174
```

Webの開発サーバーもtailnetで待ち受けます。

```powershell
npm run dev -- --host 0.0.0.0 --port 5174
```

ページは `http://TAILSCALE_IP:5174/?url=ws://TAILSCALE_IP:8787/remote&workspace=WORKSPACE_ID&token=PAIRING_TOKEN` を開きます。

`--allow-remote` を付けると、tailnet内の端末からページとBridgeへ到達できます。Origin検証は認証ではないため、Hostへの接続に必要なcredentialやpairing tokenとは別に、Tailscale ACLで到達端末を制限してください。Windowsファイアウォールで該当ポートの受信許可も必要です。

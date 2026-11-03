# Web Bridge

ブラウザーのWebSocketを、同じPC上のHostが受け付けるQUIC接続へ転送する独立実行ファイルです。認証やWorkspace操作の判断はHostが行います。

```powershell
cargo run -p lapis-web-bridge -- --listen 127.0.0.1:8787 --host 127.0.0.1:HOST_PORT --server-name lapis-host --certificate HOST_CERTIFICATE.der --origin http://localhost:5173
```

`--host` と `--certificate` は起動中Hostのアドレスと公開証明書です。証明書とサーバー名を検証し、検証失敗時に接続を許可しません。待ち受けとHost接続はloopbackに限定し、指定したOriginと `lapis.v0` subprotocolだけを受け付けます。Webページ自体は配信しません。

別PCから開く開発時は、SSHローカルポート転送でWebの開発サーバーとBridgeのTCPポートを転送し、ブラウザーから転送先のlocalhostを開きます。Hostの認証・Workspaceへの接続はClient側で別途必要です。

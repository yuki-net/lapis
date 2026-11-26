# Mobile

Kotlin Multiplatform の Mobile Client です。現在の実装は Android を対象にしています。

## OIDC ログイン

Host が OIDC を設定している場合、`AuthRequired.oidc` の issuer / client_id / scopes を使って Authorization Code + PKCE ログインを行います。

- redirect URI: `lapis://auth/callback`
- IdP には `LAPIS_OIDC_CLIENT_ID` の public client と、上記 redirect URI を登録してください。
- 認証後、Host が検証した `ServerHello.subject` ごとに ID Token を Android Keystore（AES/GCM）で暗号化し、アプリ専用の SharedPreferences に保存します。
- 次回の「OIDC で接続」では保存済み Token を先に提示し、Host が拒否した場合だけブラウザーログインへ戻ります。
- Token は URL、Cookie、ログへ出しません。
- Android の custom scheme は他アプリが横取りできる可能性があります。製品化時は App Links（検証済み HTTPS）または loopback redirect に置き換えてください。
- iOS の secure storage とログイン UI は未実装です。

# 認証とセッション

Host・Web・Desktop・Mobile の実装者が、認証方式の追加・変更時に判断するための正本。

## 役割

| 主体 | 役割 |
| --- | --- |
| IdP | 外部の OIDC Provider。Keycloak、Google、または開発用 broker。 |
| Host | Resource Server。JWT を検証し、Workspace への Session を発行する。OIDC Client にはならない。 |
| Web / Desktop / Mobile | OIDC Client。Authorization Code + PKCE で IdP から Token を取得する。 |
| broker | IdP と Client の間に置く任意の OIDC 実装。Host の裏に置き、Web は broker の存在を知らない。 |

- Web は Host 以外の URL を設定しない。IdP / broker の設定は Host が提示する。
- Host は Cookie を受け付けない。認証は WS handshake 後の `AuthRequired` と `ClientMessage` で行う。
- Token 方式と OIDC 方式を併存させる。`AuthRequired.methods` に有効な方式だけを並べる。

## 認証方式

### Token 方式（既存）

- 初回は pairing token、以後は Host が発行した credential secret を使う。
- credential は SQLite に保存し、Host は secret の digest だけを保持する。
- 接続時の `ClientMessage::Pair` / `ClientMessage::Authenticate` は現行契約を維持する。

### OIDC 方式

> 現行の Host は OIDC の認証要求を受け付けず、`AuthRequired.methods` には `Token` だけを返す。この節は実装予定の契約であり、実装が入るまで Client は OIDC を選択しない。

- Client は Authorization Code + PKCE で IdP から ID Token を取得する。
- Client は `ClientMessage::Oidc(OidcRequest)` に ID Token と Workspace を入れて送る。
- Host は Token をローカルで検証し、成功時に `ServerMessage::Authenticated(ServerHello)` を返す。
- OIDC Session は credential secret を発行しない。Session ID は bearer secret として扱わない。
- 再接続時も Client は保存した OIDC ID Token を `ClientMessage::Oidc` で再提示し、Host が再検証して新しい Session を発行する。
- Web は Token を `sessionStorage` に置く。Desktop / Mobile は OS の secure storage に置く。
- どの Client も Token を URL、Cookie、ログ、クラッシュレポートへ出さない。

## Host の検証

- 署名: IdP の JWKS を取得し、`kid` に対応する鍵で検証する。JWKS は期限付きで cache する。
- Claim: `iss`、`aud`、`exp`、`nbf`、`iat` を検証する。Clock skew は 60 秒を上限とする。
- アルゴリズム: 設定された許可リストだけを受け付ける。`none` と対称鍵アルゴリズムは既定で拒否する。
- `sub` は Host の Session 所有者として扱う。`client_id` は許可リストがある場合だけ照合する。
- 検証失敗は `UNAUTHORIZED` で接続を閉じる。Token の内容や検証理由を Client へ返さない。
- 実装は `jsonwebtoken` + JWKS 取得を第一候補とする。Host は Resource Server であり、Authorization Code の交換は行わない。

## 設定

Host の認証設定は次の値を持つ。

- `issuer`
- `audience`
- `jwks_uri`
- `allowed_algorithms`
- `subject_allowlist`（任意）
- `session_lifetime`
- `clock_skew`

設定が無い場合、Host は `AuthRequired.methods` に `Oidc` を出さない。Token 方式は設定に依存せず利用できる。

## Session と失効

- Host は Session ID、Workspace、capability、期限、認証方式を所有する。
- OIDC Session の期限は `exp` と `session_lifetime` の短い方とする。
- 切断だけでは Session を破棄しない。明示的な logout、期限切れ、Host の停止で破棄する。
- Web の再読み込みでは `sessionStorage` の Token で再接続する。Token が無効なら `AuthRequired` からやり直す。
- Desktop / Mobile は secure storage の Token で再接続する。Cookie は使わない。
- 新しい Device は既定で明示承認とする。自動承認は設定で明示した場合だけ許可する。

## 開発・テスト

- 開発用 broker は Dex を第一候補とする。製品環境では外部 IdP をそのまま使う。
- mock provider は固定の JWKS と署名鍵を持ち、issuer / audience / exp / 署名 / `kid` 不一致をテストする。
- Token 方式と OIDC 方式の両方で、接続・再接続・失効・capability 拒否をテストする。
- 実 IdP の設定値や秘密鍵はリポジトリへ置かない。

## 非対象

- Implicit flow、Password grant、Form post response mode。
- Cookie による Session 継続。
- Host 自身が IdP になること。
- OIDC broker の自作。必要になった場合は別プロセスとして追加する。

# Host

Lapisの状態・実行・外部環境への接続を扱う実装領域です。現在のcrate境界は検討中の構成であり、変更可能です。

## 役割と理由

- 複数Clientに整合して見える必要がある正規状態と、UIのみの一時的な状態を区別します。二重の正本を避けるためです。
- OS、ファイルシステム、外部CLI、通信などの具体実装を、アプリケーションの判断ロジックから必要な範囲で隔離します。LocalとRemoteで意味を共有するためです。
- 単純な転送だけの層や汎用 `manager` を増やさず、実際に異なる状態所有者・ライフサイクル・エラー回復の境界で分割します。
- Resourceの開始・終了・再接続・失敗からの回復は所有する側で一貫して扱います。

全体に関わる意味は [Architecture](../docs/ARCHITECTURE.md)、Remote固有の検討事項は [Remote internal specifications](../docs/specs/remote.md) を参照してください。

## 実装領域

- `application/`：操作判断とユースケース。
- `runtime/`：Workspace 等の正規状態と実行リソースの寿命。
- `infrastructure/`：ファイルシステム、Git CLI、PTY、Agent、永続化の具体実装。
- `transport/`：認証と通信経路。独立した正規状態を作らない。
- `host/`：Runtime と具体実装の組み立て、Desktop向けport、実行リソースの起動・終了。Desktop の接続を破棄しても Host 自体は終了しない。

依存方向は [責務と依存方向](../docs/specs/architecture.md) を参照してください。

## 独立Hostの起動

`cargo run -p host -- --workspace <既存フォルダ> --certificate <新規の公開証明書.der>`

`--state-directory <directory>` を指定すると、共有Workbench表示状態をHostの再起動後も復元します。未指定の独立Hostではメモリー内だけに保持します。管理Hostは既存のHost状態ディレクトリを使用します。

`--connection-info <file>` を指定すると、待受を開始した後に待受アドレス・server_name・Workspace ID・公開証明書を1つのJSONへ書き出します。書くのは `--certificate` が成功した後だけなので、待受に失敗した起動が稼働中Hostの情報を壊しません。tokenや秘密鍵は含めず、既存ファイルは上書きします。正常終了（`quit` / Ctrl+C）時は、書いたアドレスが自分の待受アドレスと一致する場合に限りこのファイルを削除します。Clientやbridgeへ接続先を自動で渡したい場合に使います。未指定の場合はファイルを作りません。

既定ではloopbackの空きUDP portでQUICを受け付けます。別端末から接続する場合は `--listen <ip:port>` を明示します。表示されたaddress・server_name・Workspace IDと、信頼できる経路で渡した公開証明書をClientで使用します。既存の証明書ファイルは上書きしません。

Hostの標準入力で `invite <Clientから到達可能なip:port> <新規の招待.json>` を実行すると、公開証明書・Workspace・1時間で一回限りの招待をファイルにまとめます。信頼できる経路でClientごとに渡してください。`pair` はWorkspace限定の招待token、`pair-user` はWorkspaceの一覧・切替・登録を許可するユーザー用tokenをconsoleへ表示する低水準の操作です。ユーザー用tokenにはその他のHost管理権限はありません。Host所有者は `register <directory>` で別のWorkspaceを登録できます。`devices` は接続済みDeviceの一覧、`approve <device-id>` は承認待ちDeviceの許可、`revoke-device <device-id>` はDeviceとそのcredential / OIDC Sessionの失効、`revoke <credential-id>` は資格情報の失効、`quit` またはCtrl+CはHost自体を終了します。Client切断や標準入力のEOFではHostを終了しません。

Host側の秘密鍵はメモリ内に保持します。credentialは状態ディレクトリの `credentials.sqlite3` に保存し、Host再起動後に復元します。Remote credentialの期限は1日です。状態ディレクトリを指定したHostは登録済みWorkspaceを再起動後に復元します。見つからないフォルダの登録情報は削除しません。旧形式には登録一覧がないため、既存Workspaceは次に開いたとき一覧へ記録し、従来のWorkbench状態を引き続き使います。Desktopは「ファイル → Hostに接続…」または `cargo run -p lapis -- --host <招待.jsonまたは保存済みprofile.host>` で接続します。取り込んだ証明書とcredentialはWindowsのユーザー単位DPAPIで保護して保存し、以後はprofileから接続します。認証・証明書の不一致時に自動で信頼し直しません。

Desktopの通常起動は `lapis-host.exe` の `--managed-host` modeを独立processで起動し、loopback QUICで接続します。同じユーザーのstate directoryで一つのHostを共有し、複数のフォルダを登録できます。所有者credentialは匿名pipeで渡し、DPAPIで保護した `hosts/local-v1.host` に保存します。Local所有者の鍵はHostの寿命に紐付き、Remoteの招待には配布しません。通信失敗だけで既存Hostを終了・置換しません。Desktopを閉じてもHostのDocument・Terminalは残ります。Host自体の終了後は実行状態を復元しません。

`lapis.exe`（Desktop Client）と `lapis-host.exe`（Host）は明確に分離されたbinaryです。Desktopは自身をHostとして起動せず、既定では同じ出力ディレクトリから `lapis-host.exe` を解決して起動します。`--local-host-executable <path>` を指定した場合はDesktopのComposition Rootがそのファイルを解決してLocal接続へ注入します。Hot Reload launcherはこの指定でdxの一時出力先とHostを分離します。

実通信と別processの回帰検証は `cargo test -p host --test quic --test standalone --test managed_client` で実行します。Unixの開発・CI環境ではDPAPIの代わりにdirectory 0700・file 0600を使用します。

## OIDC

次の環境変数がすべて設定された場合だけ、Host は `AuthRequired.methods` に `Oidc` を追加します。

- `LAPIS_OIDC_ISSUER`（HTTPS）
- `LAPIS_OIDC_AUDIENCE`
- `LAPIS_OIDC_CLIENT_ID`（任意、既定は audience）
- `LAPIS_OIDC_SCOPES`（任意、既定 `openid profile`）
- `LAPIS_OIDC_JWKS_FILE` または `LAPIS_OIDC_JWKS_URI`（どちらか一方）
- `LAPIS_OIDC_CLOCK_SKEW_SECONDS`（任意、既定60秒）

`LAPIS_OIDC_ISSUER` と `LAPIS_OIDC_CLIENT_ID`、`LAPIS_OIDC_SCOPES` は `AuthRequired.oidc` としてネイティブ Client へ提示します。ネイティブ Client は issuer の OIDC discovery で endpoint を解決します。OIDC 認証に成功すると、Host は `ServerHello.subject` に検証済み `sub` を返します。

Host の標準入力で `invite-oidc <Clientから到達可能なip:port> <新規の招待.json>` を実行すると、pairing token を含まない OIDC 用の招待を作成します。Desktop は `lapis --host <招待.json>` でこの招待を開き、システムブラウザーでログインします。

OIDC で初めて接続した Device は既定で承認待ちになります。Host の標準入力で `devices` を実行して device ID を確認し、`approve <device-id>` で許可します。`revoke-device <device-id>` は Device とその credential / OIDC Session を失効させます。managed Host は OS ユーザーが所有するローカル実行環境のため自動承認します。独立 Host で自動承認する場合は `LAPIS_AUTO_APPROVE_OIDC_DEVICES=1` を設定します。承認待ちの Device は `UNAUTHORIZED` で接続を拒否され、Host のログに device ID が表示されます。

Desktop の loopback redirect URI は既定で `http://127.0.0.1:53682/auth/callback` です。`LAPIS_DESKTOP_OIDC_REDIRECT_URI` で変更できます。IdP には `LAPIS_OIDC_CLIENT_ID` の public client として、この redirect URI を登録してください。

`LAPIS_OIDC_JWKS_FILE` は起動時に読み込みます。`LAPIS_OIDC_JWKS_URI` は `https://` の URI を指定し、リダイレクト先も HTTPS に限定されます。起動時に取得し、以後5分ごとに更新します。初回取得に失敗した場合、独立 Host は起動しません。起動後の定期更新に失敗した場合は既存の鍵を保持します。HTTP はテスト専用です。設定の詳細は [認証とセッション](../docs/specs/auth.md) を参照してください。

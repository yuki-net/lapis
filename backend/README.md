# Backend

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

`cargo run -p lapis-host -- --workspace <既存フォルダ> --certificate <新規の公開証明書.der>`

Projectへ複数のRepositoryや既存ソースフォルダを公開する場合は `--source <既存フォルダ>` を必要な数だけ追加します。招待はこの明示した環境だけを許可し、ClientのProject画面に「Host環境を追加」として表示します。Host上の他の登録先を自動で公開しません。

既定ではloopbackの空きUDP portでQUICを受け付けます。別端末から接続する場合は `--listen <ip:port>` を明示します。表示されたaddress・server_name・Workspace IDと、信頼できる経路で渡した公開証明書をClientで使用します。既存の証明書ファイルは上書きしません。

Hostの標準入力で `invite <Clientから到達可能なip:port> <新規の招待.json>` を実行すると、公開証明書・Workspace・5分間で一回限りの招待をファイルにまとめます。信頼できる経路でClientごとに渡してください。`pair` は招待tokenをconsoleへ表示する低水準の操作です。`revoke <credential-id>` は資格情報を失効させ、`quit` またはCtrl+CはHost自体を終了します。Client切断や標準入力のEOFではHostを終了しません。

Host側の秘密鍵とcredentialはメモリ内に保持するため、Host再起動後は新しい公開証明書とペアリングが必要です。Remote credentialの期限は1日です。Desktopは「ファイル → Hostに接続…」または `cargo run -p lapis -- --host <招待.jsonまたは保存済みprofile.host>` で接続します。取り込んだ証明書とcredentialはWindowsのユーザー単位DPAPIで保護して保存し、以後はprofileから接続します。認証・証明書の不一致時に自動で信頼し直しません。

Desktopの通常起動は同じ実行ファイルの `--managed-host` modeを独立processで起動し、loopback QUICで接続します。同じユーザーのstate directoryで一つのHostを共有し、複数のフォルダを登録できます。所有者credentialは匿名pipeで渡し、DPAPIで保護した `hosts/local-v1.host` に保存します。Local所有者の鍵はHostの寿命に紐付き、Remoteの招待には配布しません。通信失敗だけで既存Hostを終了・置換しません。Desktopを閉じてもHostのDocument・Terminalは残ります。Host自体の終了後は実行状態を復元しません。

実通信と別processの回帰検証は `cargo test -p lapis-host --test quic --test standalone --test managed_client` で実行します。Unixの開発・CI環境ではDPAPIの代わりにdirectory 0700・file 0600を使用します。

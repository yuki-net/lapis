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
- `host/`：Runtime と具体実装の組み立て、埋め込み接続の生成、実行リソースの起動・終了。Desktop の接続を破棄しても Host 自体は終了しない。

依存方向は [責務と依存方向](../docs/specs/architecture.md) を参照してください。

## 独立Hostの起動

`cargo run -p lapis-host -- --workspace <既存フォルダ> --certificate <新規の公開証明書.der>`

既定ではloopbackの空きUDP portでQUICを受け付けます。別端末から接続する場合は `--listen <ip:port>` を明示します。表示されたaddress・server_name・Workspace IDと、信頼できる経路で渡した公開証明書をClientで使用します。既存の証明書ファイルは上書きしません。

Hostの標準入力で `pair` を実行すると、5分間・一回限りの招待を表示します。Clientごとに発行してください。`revoke <credential-id>` は資格情報を失効させ、`quit` またはCtrl+CはHost自体を終了します。Client切断や標準入力のEOFではHostを終了しません。

秘密鍵と資格情報はメモリ内に保持するため、Host再起動後は新しい公開証明書とペアリングが必要です。Desktopの通常起動は引き続き埋め込みHostを使用しており、この実行ファイルへの接続UIは未実装です。実通信と別プロセスの回帰検証は `cargo test -p lapis-host --test quic --test standalone` で実行します。

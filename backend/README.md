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
- `host/`：起動・終了・依存注入（必要になったときに実装）。

依存方向は [責務と依存方向](../docs/specs/architecture.md) を参照してください。

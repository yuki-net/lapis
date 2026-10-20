# Shared

Client と Backend の双方が利用する型・操作契約を置く領域です。

- `models/`：通信・画面・OS に依存しない値、ドメイン規則、抽象 Port。状態の所有者や実行ロジックをここに増やさない。
- `protocol/`：Request・Response・Event、Revision、Capability 等の公開契約。内部の OS パスや UI 状態をそのまま公開しない。

具体実装は Backend または Client 側で持ち、共通コードへ GPUI・外部プロセス・ネイティブダイアログの依存を持ち込まないでください。

配置と依存方向は [責務と依存方向](../docs/specs/architecture.md) を参照してください。

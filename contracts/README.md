# Contracts

Client と Host の双方が利用する型・操作契約を置く領域です。

- `domain/`：通信・画面・OS に依存しない値、ドメイン規則、抽象 Port。状態の所有者や実行ロジックをここに増やさない。Terminal の型と `TerminalBackend` port は拡張の契約（`extensions/terminal/contract`）が正本で、ここには置かない。拡張の UI 宣言の型は `contracts/domain/extension-ui` が正本で、GPUI の View 生成や配置の解決は持たない。
- `protocol/`：Request・Response・Event、Revision、Capability 等の公開契約。内部の OS パスや UI 状態をそのまま公開しない。wire DTO の正本は Rust 側で、Mobile の Kotlin DTO は `protocol/tests/wire_fixtures.rs` と同じ JSON をテストで検証する。
- `wire/`：Transport 固有の wire DTO。domain / protocol の公開契約をそのまま通信形式にしない。

具体実装は Host または Client 側で持ち、共通コードへ GPUI・外部プロセス・ネイティブダイアログの依存を持ち込まないでください。Client の共通 transport は `clients/transport/` に置きます。拡張として自己完結させる機能の契約は `extensions/<機能>/contract/` に置きます。

配置と依存方向は [責務と依存方向](../docs/specs/architecture.md) を参照してください。

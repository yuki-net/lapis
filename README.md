# Lapis

Lapis は、Rust backend を GPUI Desktop と KMP Mobile（Android・iOS）から利用する開発 Workspace です。Desktop は local backend を同一プロセスで実行し、Mobile は remote 接続します。Web client は現在の対象に含みません。

## 責務

- `apps/`: 画面と client 固有の状態を管理します。
- `backend/`: Workspace、Document、Terminal などの正規状態と、local・remote 環境への接続を管理します。
- `features/`: Git、LSP、Terminal など、独立して有効化できる機能を配置します。
- `vendor/`: 外部コードを配置します。

設計境界と依存方向は [`ARCHITECTURE.md`](ARCHITECTURE.md) を参照してください。

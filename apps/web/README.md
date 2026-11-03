# Web placeholder

現在のWeb Clientは、今後の検討用の雛形です。DesktopやMobileの変更を自動で移植する対象ではありません。

Webの追加実装、依存更新、ビルド対象の変更は、依頼範囲と現行CIの事情に応じて判断してください。Web向けに未実装の機能が利用可能だと記載しないでください。

`wasm/` にはHostの認証と共通request/eventを扱うBrowser WebSocket接続があります。現時点ではページやDesktop UIには接続していません。

通信の結合検証にはNode.js 24と `wasm-bindgen-cli` 0.2.129が必要です。隔離した一時WorkspaceでHostとweb-bridgeを起動し、ユーザー用接続のWorkspace切替と権限、2つのWasm接続間でのWorkbench更新とrevision競合、全接続の切断後に未保存文書とTerminalを再取得できることを確認します。

```powershell
cargo build -p lapis-host -p lapis-web-bridge
cargo build -p lapis-web --target wasm32-unknown-unknown --profile web --features bridge-smoke
python apps/web/wasm/tests/bridge_smoke.py
```

# Web Client

ブラウザーから、同じPC上のHostが持つ作業状態をDesktopと共有して継続するためのClientです。DesktopやMobileの変更を自動で移植する対象ではありません。

`wasm/` はHostの認証、共通request/event、Workbench状態を扱うBrowser WebSocket接続です。`src/main.ts` はページからWasm Clientを呼び出し、共有Workbenchの文書と未保存下書きを表示・編集します。

## 前提と制限

- 単一ユーザー・設定済みHostを前提とし、ログイン画面はありません。
- 初回はHostが発行するpairing tokenで接続し、以後はlocalStorageのcredentialで再接続します。
- credentialはブラウザーのlocalStorageに保存します。共用端末では使い終わったらサイトデータを削除してください。
- Hostを再起動するとHost側のcredentialが消えるため、再度pairing tokenが必要です。
- 現時点のページは共有Workbench（開いている文書、パス、未保存下書き、カーソル位置）の表示と編集までです。Desktopの全機能（Terminal、Git、Taskなど）はまだページから操作できません。

## 開発

```powershell
cd apps/web
npm ci
npm run dev
```

`npm run dev` と `npm run build` は先に `wasm-bindgen` でWasmを生成します。Node.js 24と `wasm-bindgen-cli` 0.2.129が必要です。

Hostとweb-bridgeを起動し、ページのURL・Workspace ID・pairing tokenを入力して接続します。URLとWorkspaceはクエリ文字列でも指定できます。

```text
http://127.0.0.1:5173/?url=ws://127.0.0.1:8787/remote&workspace=<id>&token=<token>
```

## 結合検証

隔離した一時WorkspaceでHostとweb-bridgeを起動し、ユーザー用接続のWorkspace切替と権限、2つのWasm接続間でのWorkbench更新とrevision競合、全接続の切断後に未保存文書とTerminalを再取得できることを確認します。

```powershell
cargo build -p lapis-host -p lapis-web-bridge
cargo build -p lapis-web --target wasm32-unknown-unknown --profile web --features bridge-smoke
python apps/web/wasm/tests/bridge_smoke.py
```

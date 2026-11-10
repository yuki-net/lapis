# Web Client

Desktop と同じ Rust/GPUI の UI を WebAssembly でブラウザーに表示します。ページの HTML は GPUI を起動するための最小限のシェルだけです。

`clients/ui` が GPUI の起動と Wasm エントリを持ち、`clients/web` はその Wasm を読み込んで配信します。Host へ接続した場合は、ファイルの読み込み・編集・保存を Host 経由で行います。

## Host へ接続する

URL と Workspace ID をクエリか localStorage に渡します。OIDC を使う場合はログインフォームから IdP で認証します。OIDC を使わない場合は、初回は pairing token、以降は保存済み credential で接続します。

```text
http://127.0.0.1:4747/?url=ws://127.0.0.1:4757/remote&workspace=WORKSPACE_ID&token=PAIRING_TOKEN
```

Host なしのデモ:

```text
http://127.0.0.1:4747/?demo=1
```

URL / Workspace が無い場合は、Host URL・Workspace ID・pairing token を入力するフォームが表示されます。Host URL には bridge の既定（`ws://<hostname>:4757/remote`）があらかじめ入ります。OIDC が設定された Web bridge では「OIDC でログイン」から IdP のログインを開けます。取得した ID Token は `sessionStorage` に置き、再読み込み時の再接続に使います。

起動手順・ポート・ログインの詳細は [開発時の起動](../../docs/dev.md) を参照してください。ログインフォームの Host URL には **bridge** の WebSocket（`ws://127.0.0.1:4757/remote`）を入力します。Host の QUIC アドレスではありません。

- 接続すると Workspace のファイルツリーを Host から取得し、`README.md`（無ければ最初のファイル）を開きます。
- 編集はローカルへ即時反映し、Host へ非同期で送ります。保存は Host の `document.save` を使います。
- `?demo=1` を明示した場合だけ、メモリー上のサンプル Workspace で起動します（再読み込みで初期化）。

## 現状の制限

- Host 接続時、起動時に開くのは1ファイルだけです。他のファイルは開いた直後は空で、Host からの読み込みが終わると次の再取得で表示されます。
- Workbench は Host へ保存し、接続時に開いていた文書・未保存下書き・パネル配置を復元します。
- Terminal は Host へ接続して開始・入力・リサイズ・終了・出力の受信まで行います。ページ再読み込み後は Host の snapshot から既存 Terminal を再発見できます。
- Git、LSP、Search、Task は未接続です。
- Host のイベント購読は Terminal のみで、他の Client の文書変更はこちらから操作したときに再取得されます。

## 開発

```powershell
cd clients/web
npm ci
```

`Run Web`（ページ側は `cargo run -p lapis-web -- --dev --dist clients/web/dist`）が `4747` で待ち受け、起動時に `npm run build` で `clients/web/dist` を作ります。Host と bridge を含めた起動手順・ポートの決め方は [開発時の起動](../../docs/dev.md) を参照してください。

`npm run build` は Wasm を生成し、`dist` に HTML・JS・CSS・Wasm を揃えます。Node.js 24（`tsc` 用）と `wasm-bindgen-cli` 0.2.129 が必要です。型チェックだけを行う場合は `npm run typecheck` を使います。

`lapis-web --dev` 中は `clients/ui`・`clients/state`・`clients/transport`・`contracts`・`extensions` 配下の `.rs`、`clients/web/src`、`clients/web/index.html` を変更すると `npm run build` が走り、ブラウザーが再読み込みします（初回は数十秒、以降は増分で数秒）。`clients/web/src/generated` と `dist` は監視しません。再ビルドは開発サーバーだけの機能で、`npm run build` は1回だけ生成します。

WebGPU が使えない環境では `?backend=webgl` を付けてください。

## Tailscaleで開く

```powershell
cargo run -p lapis-web -- --dev --listen 0.0.0.0:4747 --dist clients/web/dist
```

`http://TAILSCALE_IP:4747/` を開きます。Host と web-bridge も tailnet から到達できるように起動してください。OIDC を使わない場合は pairing token が必要です。Tailscale ACL で端末を制限してください。

## 結合検証（Host / Bridge）

```powershell
cargo build -p host -p web-bridge
cargo build -p web --target wasm32-unknown-unknown --profile web --features bridge-smoke
python clients/web/wasm/tests/bridge_smoke.py
```

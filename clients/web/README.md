# Web Client

Desktop と同じ Rust/GPUI の UI を WebAssembly でブラウザーに表示します。ページの HTML は GPUI を起動するための最小限のシェルだけです。

`clients/ui` が GPUI の起動と Wasm エントリを持ち、`clients/web` はその Wasm を読み込んで配信します。Host へ接続した場合は、ファイルの読み込み・編集・保存を Host 経由で行います。

## Host へ接続する

URL と Workspace ID をクエリか localStorage に渡します。初回は pairing token、以降は保存済み credential で接続します。

```text
http://127.0.0.1:5174/?url=ws://127.0.0.1:8787/remote&workspace=WORKSPACE_ID&token=PAIRING_TOKEN
```

Host なしのデモ:

```text
http://127.0.0.1:5174/?demo=1
```

URL / Workspace が無い場合は、Host URL・Workspace ID・pairing token を入力するフォームが表示されます。OIDC が設定された Web bridge では「OIDC でログイン」から IdP のログインを開けます。取得した ID Token は `sessionStorage` に置き、再読み込み時の再接続に使います。

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
npm run dev
```

`npm run dev` と `npm run build` は先に `wasm-bindgen` で Wasm を生成します。Node.js 24 と `wasm-bindgen-cli` 0.2.129 が必要です。

WebGPU が使えない環境では `?backend=webgl` を付けてください。

## Tailscaleで開く

```powershell
cd clients/web
npm run dev -- --host 0.0.0.0 --port 5174
```

`http://TAILSCALE_IP:5174/` を開きます。Host と web-bridge も tailnet から到達できるように起動してください。OIDC を使わない場合は pairing token が必要です。Tailscale ACL で端末を制限してください。

## 結合検証（Host / Bridge）

```powershell
cargo build -p host -p web-bridge
cargo build -p web --target wasm32-unknown-unknown --profile web --features bridge-smoke
python clients/web/wasm/tests/bridge_smoke.py
```

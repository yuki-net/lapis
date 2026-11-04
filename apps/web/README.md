# Web Client

Desktop と同じ Rust/GPUI の UI を WebAssembly でブラウザーに表示する試作です。ページの HTML は GPUI を起動するための最小限のシェルだけです。

`wasm/` は `lapis-desktop-ui` の `run_web` を呼び、GPUI の canvas をページへ描画します。ファイルと設定はメモリー上にあり、Host や Desktop の実データにはまだ接続していません。

## 現状

- 表示・編集・タブ・ファイルツリー・コマンドパレットなど、Desktop の GPUI UI が動きます。
- ファイルと設定はページ内のメモリーだけです。再読み込みで初期状態に戻ります。
- Terminal、Git、LSP、Task runner は未接続です（呼ぶと未対応エラーになります）。
- Host の共有 Workbench への接続はまだ配線していません。`wasm/src/session.rs` に Host 接続用の非同期 API がありますが、GPUI UI の同期 Client からは未使用です。

## 開発

```powershell
cd apps/web
npm ci
npm run dev
```

`npm run dev` と `npm run build` は先に `wasm-bindgen` で Wasm を生成します。Node.js 24 と `wasm-bindgen-cli` 0.2.129 が必要です。

ブラウザーで開くと自動的に起動します。WebGPU が使えない環境では `?backend=webgl` を付けてください。

## Tailscaleで開く

同じ tailnet 内の端末から直接開けます。

```powershell
cd apps/web
npm run dev -- --host 0.0.0.0 --port 5174
```

`http://TAILSCALE_IP:5174/` を開きます。ページ自体にログインはないため、tailnet 内の到達端末は Tailscale ACL で制限し、Windows ファイアウォールで 5174 の受信を許可してください。

## 結合検証（Host / Bridge）

Host と web-bridge の通信、Workspace 切替、文書の競合、再接続は `bridge_smoke.py` で検証します。GPUI UI の起動とは独立しています。

```powershell
cargo build -p lapis-host -p lapis-web-bridge
cargo build -p lapis-web --target wasm32-unknown-unknown --profile web --features bridge-smoke
python apps/web/wasm/tests/bridge_smoke.py
```

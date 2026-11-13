# Web Client

Desktop と同じ Rust/GPUI の UI を WebAssembly でブラウザーに表示します。ページの HTML は GPUI を起動するための最小限のシェルだけです。

`clients/ui` が GPUI の起動と Wasm エントリを持ち、`clients/web` はその Wasm を読み込んで配信します。Host へ接続した場合は、ファイルの読み込み・編集・保存を Host 経由で行います。

## Host へ接続する

接続状態は URL ではなくブラウザーの保存領域（`localStorage` / `sessionStorage`）に置きます。URL は開いている状態だけを表します。

- `/` は保存済みの Host / Workspace 一覧です。初回は Desktop で発行した招待リンクを開くか、画面に貼り付けます。
- `/<host>/<workspace>` はその Workspace を開いた状態です。パスのセグメントは表示用ラベルで、接続先 URL と Workspace ID は保存済みの対応表から引きます。
- 一覧の「このブラウザーから接続情報を削除」はローカルの credential と対応表を消します。Host 側の端末失効は Desktop の設定で行います。
- `#/t/<task-id>` は選択する Task です。選択に合わせて `history.replaceState` で更新し、履歴は増やしません。
- Workspace の切替はアプリ内の Workspace メニューから行います。同じ接続のまま `workspace.close` → `workspace.connect` を実行し、ページ遷移・再接続はしません。URL も `history.replaceState` で `/<host>/<workspace>` に追従し、`#/t/<task-id>` は保持します。`workspace.switch` がない接続では切替メニューを表示しません。
- ページの生存中は Workspace ごとの表示状態を保持します。一度開いた Workspace に戻ると、保持した状態を復元してから `snapshot.resync` と Workbench の再取得で差分を反映します。保持は接続中の Workspace 一覧数を上限とし、超えた分は古いものから破棄して次回は snapshot から復帰します。`localStorage` へは保存しません。
- 招待リンクは `#/invite?token=<token>` の形式です。Web は同じアドレスの `/remote` に接続し、Workspace は Host が決定します。token は fragment のみに置き、読み取り後にアドレスから消します。

Host なしのデモ:

```text
http://127.0.0.1:4747/?demo=1
```

OIDC が設定された Host では「OIDC でログイン」から IdP のログインを開けます。取得した ID Token は `sessionStorage` に置き、再読み込み時の再接続に使います。

旧形式の `?url=` / `?workspace=` / `?token=` や、接続先と Workspace を含む fragment は接続に使いません。Desktop から新しい招待を発行してください。

起動手順・ポート・ログインの詳細は [開発時の起動](../../docs/dev.md) を参照してください。

- 接続すると Workspace のファイルツリーを Host から取得し、`README.md`（無ければ最初のファイル）を開きます。
- 編集はローカルへ即時反映し、Host へ非同期で送ります。保存は Host の `document.save` を使います。
- `?demo=1` を明示した場合だけ、メモリー上のサンプル Workspace で起動します（再読み込みで初期化）。

## Workspace の view state 共有

同じ Workspace を開いている別 Client と、開いているタブの構成とアクティブ文書を共有します。契約の正本は [Workspace の view state 共有](../../docs/specs/remote.md#workspace-の-view-state-共有) です。

- Workspace へ接続すると `workspace.view.subscribe` で購読し、snapshot をタブ構成へ適用します。以後の `workspace.view_changed` は delta として適用します。
- 自分のタブ開閉とアクティブ文書の変更は 150ms の debounce で `workspace.view.update` を送ります。直前の snapshot で適用したタブ構成と同じなら送りません。revision / sequence により、自分の更新が event として戻っても再適用・再送しません。`view` が `null` の event は無視します。
- タブをバックグラウンドから戻したときと再接続時は、snapshot を取り直して再同期します。切断中は古い共有状態であることをステータスバーに表示します。
- Workspace を閉じたときとページ終了時は `workspace.view.unsubscribe` を送り、以後の delta を受け取りません。
- 共有するのはタブ構成と文書の更新状況だけで、パネル寸法・カーソル位置・スクロールは送らず、適用もしません。未保存の文書は共有側で閉じられていてもタブとして残します。
- 既存の Workbench 同期と Task 共有の挙動は変わりません。

## 現状の制限

- Host 接続時、起動時に開くのは1ファイルだけです。他のファイルは開いた直後は空で、Host からの読み込みが終わると次の再取得で表示されます。
- Workbench は Host へ保存し、接続時に開いていた文書・未保存下書き・パネル配置を復元します。
- Terminal は Host へ接続して開始・入力・リサイズ・終了・出力の受信まで行います。ページ再読み込み後は Host の snapshot から既存 Terminal を再発見できます。
- Git、LSP、Search、Task は未接続です。
- Host のイベント購読は Terminal・Document・Workspace view state です。他の Client の文書変更は、イベントと操作時の再取得で反映されます。

## フォント

Web の GPUI は空の fontdb から起動し、ブラウザーや OS のフォントを引けません。使えるのは `clients/ui` が同梱して読み込むフォントだけです。

- UI テキストは `clients/ui/assets/IBMPlexSans-*.ttf`。
- Terminal は罫線・ブロック要素・点字を等幅で描く必要があるため、`clients/ui/assets/CascadiaMono-*.ttf` を同梱し、`clients/ui/src/bundled_font.rs` で読み込みます。Desktop (Windows) の既定と同じ family です。

ブラウザーや OS のフォントに頼る chain は Web では成立しません。`monospace` のような総称名は解決できず、読み込んだ UI フォントへ落ちて等幅でなくなり、罫線・ブロック要素が豆腐 (□) になります。そのため Web の Terminal の family は同梱フォントへ固定します。同梱フォントが family 名・グリフ被覆・等幅性を満たすことは `clients/ui` のテストで検証します。

## 開発

```powershell
cd clients/web
npm ci
```

`npm run build` で `clients/web/dist` を作成してから、Desktop の Web 接続設定で公開します。`Run Web (Page)` は Host に接続しないデモの確認用です。起動手順は [開発時の起動](../../docs/dev.md) を参照してください。

`npm run build` は Wasm を生成し、`dist` に HTML・JS・CSS・Wasm を揃えます。Node.js 24（`tsc` 用）と `wasm-bindgen-cli` 0.2.129 が必要です。型チェックだけを行う場合は `npm run typecheck` を使います。

`lapis-web --dev` の監視と再読み込みは、Host に接続しないデモ用です。公開中の Web に変更を反映するには `npm run build` を再実行し、ブラウザーを再読み込みします。

WebGPU が使えない環境では `?backend=webgl` を付けてください。

## Tailscaleで開く

Desktop の設定で Tailscale のアドレスを選んで公開し、コピーした招待リンクを別端末で開きます。Tailscale ACL で到達できる端末を制限してください。

## 結合検証（Host / Bridge）

```powershell
cargo build -p host -p web-bridge
cargo build -p web --target wasm32-unknown-unknown --profile web --features bridge-smoke
python clients/web/wasm/tests/bridge_smoke.py
```

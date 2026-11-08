# 開発時の起動

Windows で Desktop と Web を起動して確認する手順の正本です。押すボタン、ポート、保存先、ログイン手順はこのページにだけ書き、各領域の `README.md` はここを参照します。

## 入口は3つ

| 押すもの | 起動するもの |
| --- | --- |
| `Run Host` | Host だけ。Desktop と Web の両方がここへ繋がる |
| `Run Desktop` | Desktop だけ。Host の招待を待って接続する |
| `Run Web` | bridge（TCP `4748`）＋ ページ配信（TCP `4747`）。Host は起動しない |

3つは独立していて、起動順は問いません。`Run Host` を1つ立ち上げれば、Desktop と Web が同じ Host に同時に繋がります。

`.run` のその他は、`Run Web` の部品（`Run Web Bridge` / `Run Web Page`）、`Run Hot Reload`、`Build Web`、`Run Android`、`Build Android` です。部品は単体で動きを調べたいときだけ押します。

## Host と Desktop の接続

`Run Host` は待受を始めた後に2つのファイルを書きます。

| ファイル | 中身 | 読む側 |
| --- | --- | --- |
| `target/dev-web/connection.json` | 待受アドレス・サーバー名・Workspace ID・公開証明書 | bridge |
| `target/dev-desktop/desktop.json` | 上記に加えて1時間・1回限りの token | Desktop |

どちらも token の扱いが違うだけで、指す Host は同じです。Host を正常終了すると自分が書いたファイルを削除します。

`Run Desktop` は `--host target/dev-desktop/desktop.json` で起動し、招待が無ければ最大15秒待ちます。一度ペアリングすると `<state>/hosts/` に profile が残り、同じ Host へは profile で再接続するため、2回目以降は使用済みの token を使いません。

`--state-directory` を指定した Host は秘密鍵と証明書を `<state>/identity` に保存して再利用するため、Host を再起動しても Desktop の profile と Web の credential は無効になりません。

Desktop の接続先は `--host`、環境変数 `LAPIS_HOST_FILE`、ローカル Host の自動起動の順に解決します。どれも無ければ従来どおり自分で Local Host を起動するため、製品の通常起動と Run で経路は分かれません。

## Desktop

| 操作 | 手段 |
| --- | --- |
| ローカル Host を自動起動して起動 | `cargo dev` |
| `Run Host` に繋いで起動 | `Run Desktop`、または `LAPIS_HOST_FILE` を設定して `cargo dev` |
| ファイルを開いて起動 | `cargo dev --open-file README.md` |
| Hot Reload で起動 | `Run Hot Reload` または `cargo dev-hot-reload` |

```powershell
# Run Host に繋ぎたいとき（Run Desktop と同じ動き）
$env:LAPIS_HOST_FILE = "target/dev-desktop/desktop.json"; cargo dev
```

`cargo dev` も `Run Desktop` も Desktop と Host の両方をビルドします。2段階ビルドはありません。`Run Web Host` だけは Desktop を起動しないため Host だけをビルドし、Desktop のビルドが壊れていても Web 用 Host は起動できます。

Run は Windows の Job Object で、その Run が起動した process をまとめて管理します。Stop は Run が起動した process だけを終了し、他の Run や既存の Host には影響しません。`cargo dev` や製品の通常起動では Desktop を閉じても Local Host は残るため、残した場合はログの `Desktop session:` の名前を使って `powershell -NoProfile -File scripts/stop_dev_host.ps1 -Session <session名>` で停止します。

Hot Reload は次を守ります。

- 修正版 `dx`（`target/dx-fixed0710/bin/dx.exe`）が必要です。Dioxus PR #5967 がマージされるまでの暫定です。
- 対象は Windows x64 の Debug のみで、Release には Hot Reload の runtime を含めません。
- 構造体のレイアウト変更、埋め込みアセットの変更、`request_layout` / `prepaint` / `paint` を自前実装した `Element` は反映されません。再起動してください。
- devserver への接続は起動時の1回だけです。切れたら `dx` とアプリを再起動します。
- `dx` の app 出力は worktree で共有するため、同じ worktree で同時に1つだけ起動できます（`target/hot-reload.lock` で排他）。

## Web

Web はプロトコルが異なる3つの process に分かれます。Desktop は不要です。

| process | 役割 | 待受 |
| --- | --- | --- |
| Host（`lapis-host`） | Workspace と認証の本体 | QUIC。既定 `4749` |
| bridge（`lapis-web-bridge`） | ブラウザーの WebSocket を Host の QUIC へ転送し、OIDC の token 交換を行う | TCP `4748` |
| ページ配信（vite） | Wasm UI を配る | TCP `4747` |

初回だけ `cd clients\web` で `npm ci` を実行します。`npm run dev` は起動時に Wasm をビルドするため、`wasm32-unknown-unknown`、`wasm-bindgen-cli` 0.2.129、Node.js 24 が必要です。

起動順は問いません。Host は待受を始めた後に接続情報を `target/dev-web/connection.json` へ書き出し、bridge は起動時にそれが現れるまで待ち（10 秒ごとに待機中を表示）、接続のたびに読み直します。Host の初回ビルドに時間がかかっても bridge は終了しません。Host を正常終了すると、自分が書いた接続情報だけが削除されます。

### ログイン

1. `Run Host` を押し、コンソールで `pair-user` を打って token を控えます。
2. `Run Web` を押すと、bridge のログに `open http://127.0.0.1:4747/?url=ws://127.0.0.1:4748/remote&workspace=<ID>` が出ます。その URL を開きます。
3. Host URL は bridge の `ws://127.0.0.1:4748/remote`、Workspace ID は接続情報の `workspace_id` か Host ログの `workspace=`、pairing token は手順1の値です。
4. 一度接続すると credential がブラウザーの `localStorage` に保存され、次回は token 欄が空でも接続できます。

`?demo=1` を付けると Host なしのサンプル Workspace で UI だけを確認できます。

### ポートの決め方

| 用途 | 値 | 理由 |
| --- | --- | --- |
| ページ | `4747` 固定 | ブラウザーで開く URL。`strictPort` で衝突時は明示的に失敗する |
| bridge | `4748` 固定 | ログインフォームに入力する URL |
| Host | `4749` 固定 | Desktop の保存済み profile と Workspace ID が変わらないようにする |

bridge は CLI 引数、環境変数、既定値の順に設定を解決します。`--listen` / `--origin` を省くと `LAPIS_WEB_BRIDGE_LISTEN`（既定 `127.0.0.1:4748`）と `LAPIS_WEB_BRIDGE_ORIGIN`（既定 `http://127.0.0.1:4747`）を使います。

`4749` が他の用途で埋まっているときは `Run Host` の `--listen` を変えてください。その場合、Desktop は招待を作り直せば接続できます。

## Web の UI を確認する

`npm run dev` を立ち上げたまま `clients/ui`・`clients/state`・`clients/transport`・`contracts`・`extensions` 配下の `.rs` を保存すると、Wasm が自動で作り直され、ブラウザーが再読み込みします。`localStorage` の credential で再接続するため、ログインし直す必要はありません。

- 反映は再読み込みで行います。Desktop の Hot Reload のように State を保ったまま差し替えるものではありません。
- 初回と依存 crate を変えたときは数十秒かかります。2回目以降は cargo の増分ビルドで数秒です。
- 250ms 以内の連続保存は1回にまとまり、ビルド中に保存した分は終わってから1回だけ追加で走ります。ビルドは常に1本です。
- 失敗すると vite のコンソールに `web: wasm build failed` とエラー出力が出て、古い Wasm を配り続けます。画面は壊れず、修正して保存すれば再試行します。
- 監視するのは上記5ディレクトリの `.rs` だけです。`vendor/`（patched GPUI）と `apps/` を変えたときは `npm run wasm` を手で実行してください。
- 再ビルドは開発サーバーだけの機能です。`npm run build` と CI は従来どおり起動時に1回だけ生成します。

## 検証

| 対象 | コマンド |
| --- | --- |
| Desktop | `python scripts/check.py desktop` |
| Host | `python scripts/check.py host` |
| 全体とアーキテクチャ | `python scripts/check.py repository architecture` |
| Web のビルド | `cd clients\web; npm run build` |
| Host と bridge の結合 | `cargo build -p host -p web-bridge` → `python clients/web/wasm/tests/bridge_smoke.py` |
| launcher と Hot Reload | `python -m unittest discover -s scripts -p test_dev_session.py` |

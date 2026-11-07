# 開発時の起動

Windows で Desktop と Web を起動して確認する手順の正本です。押すボタン、ポート、保存先、ログイン手順はこのページにだけ書き、各領域の `README.md` はここを参照します。

## 入口は3つ

| 押すもの | 起動するもの |
| --- | --- |
| `Lapis` | Desktop ＋ 自動起動の Local Host |
| `Lapis Hot Reload` | 修正版 `dx` ＋ Local Host |
| `Lapis Web` | Host ＋ bridge（`4748`）＋ ページ配信（`4747`） |

`.run` の `Lapis Web Host` / `Lapis Web Bridge` / `Lapis Web Page` は `Lapis Web` の構成要素です。単体で動きを調べたいときだけ押します。

CLI では `cargo dev` / `cargo dev-hot-reload` が同じ launcher（`scripts/dev-session`）を通ります。Run から起動した場合は `target/dev-sessions/<session>/` に binary と状態を分離し、CLI からは `target/dev-state` を明示して状態を永続化します。

## Desktop

| 操作 | コマンド |
| --- | --- |
| ビルドして起動 | `cargo dev` |
| ファイルを開いて起動 | `cargo dev --open-file README.md` |
| Hot Reload で起動 | `cargo dev-hot-reload` または `Lapis Hot Reload` |
| Host だけ起動 | `Lapis Host` |

`cargo dev` も `Lapis` も Desktop と Host の両方をビルドします。2段階ビルドはありません。

`Lapis` は Windows の Job Object で Run が起動した process をまとめて管理し、Stop で Desktop と Local Host の両方を終了します。Desktop を閉じただけでは Local Host は残るため、残した場合はログの `Desktop session:` の名前を使って `powershell -NoProfile -File scripts/stop_dev_host.ps1 -Session <session名>` で停止します。

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
| Host（`lapis-host`） | Workspace と認証の本体 | QUIC。既定は空きポートの自動割当 |
| bridge（`lapis-web-bridge`） | ブラウザーの WebSocket を Host の QUIC へ転送し、OIDC の token 交換を行う | TCP `4748` |
| ページ配信（vite） | Wasm UI を配る | TCP `4747` |

初回だけ `cd clients\web` で `npm ci` を実行します。`npm run dev` は起動時に Wasm をビルドするため、`wasm32-unknown-unknown`、`wasm-bindgen-cli` 0.2.129、Node.js 24 が必要です。

起動順は問いません。Host は待受を始めた後に接続情報を `target/dev-web/connection.json` へ書き出し、bridge は起動時に最大15秒それを待ち、接続のたびに読み直します。Host を正常終了すると、自分が書いた接続情報だけが削除されます。

### ログイン

1. `Lapis Web` を押し、コンソールで `pair-user` を打って token を控えます。
2. bridge のログに出る `open http://127.0.0.1:4747/?url=ws://127.0.0.1:4748/remote&workspace=<ID>` を開きます。
3. Host URL は bridge の `ws://127.0.0.1:4748/remote`、Workspace ID は接続情報の `workspace_id` か Host ログの `workspace=`、pairing token は手順1の値です。
4. 一度接続すると credential がブラウザーの `localStorage` に保存され、次回は token 欄が空でも接続できます。

`?demo=1` を付けると Host なしのサンプル Workspace で UI だけを確認できます。

### ポートの決め方

| 用途 | 値 | 理由 |
| --- | --- | --- |
| ページ | `4747` 固定 | ブラウザーで開く URL。`strictPort` で衝突時は明示的に失敗する |
| bridge | `4748` 固定 | ログインフォームに入力する URL |
| Host | 自動割当 | ブラウザーから直接叩かない。実値は接続情報とログに出る |

bridge は CLI 引数、環境変数、既定値の順に設定を解決します。`--listen` / `--origin` を省くと `LAPIS_WEB_BRIDGE_LISTEN`（既定 `127.0.0.1:4748`）と `LAPIS_WEB_BRIDGE_ORIGIN`（既定 `http://127.0.0.1:4747`）を使います。

## Web の UI だけを素早く確認する

Web の UI は `clients/ui` の Rust を Wasm にしたもので、vite は起動時に1回しかビルドしません。自動の再ビルドは無いため、UI を変えたら `npm run wasm` を実行してブラウザーを再読み込みします。Desktop のようにファイル保存だけで反映する手段はありません。

## 検証

| 対象 | コマンド |
| --- | --- |
| Desktop | `python scripts/check.py desktop` |
| Host | `python scripts/check.py host` |
| 全体とアーキテクチャ | `python scripts/check.py repository architecture` |
| Web のビルド | `cd clients\web; npm run build` |
| Host と bridge の結合 | `cargo build -p host -p web-bridge` → `python clients/web/wasm/tests/bridge_smoke.py` |
| launcher と Hot Reload | `python -m unittest discover -s scripts -p test_dev_session.py` |

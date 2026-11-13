# 開発時の起動

Windows で Desktop と Web を起動して確認する手順の正本です。押すボタン、ポート、保存先、ログイン手順はこのページにだけ書き、各領域の `README.md` はここを参照します。

## 入口は3つ

| 押すもの | 起動するもの |
| --- | --- |
| `Run Host` | Host だけ。Desktop と Web の両方がここへ繋がる |
| `Run Desktop` | Desktop だけ。Host の招待を待って接続する |
| `Run Web` | 別起動 Host の bridge と Web ページ（TCP `4747`）。事前に Web のビルドが必要 |

通常の Web 接続は `cargo dev` の Local Host と Desktop の設定から利用します。別起動 Host の招待発行は Desktop 設定では扱いません。

`Run Desktop (Hot Reload)` は `Run Desktop` の起動方法だけを変えたもので、Host の招待待ちと `target/dev-desktop/state` は共通です。

`.run` のその他は、`Run Web (Bridge)`、デモ用の `Run Web (Page)`、`Build Web`、`Run Android`、`Build Android` です。`Run Web (Page)` だけを起動して `http://127.0.0.1:4747/?demo=1` を開くと、Host と bridge なしで UI を確認できます。

## Host と Desktop の接続

`Run Host` は待受を始めた後に2つのファイルを書きます。

| ファイル | 中身 | 読む側 |
| --- | --- | --- |
| `target/dev-web/connection.json` | 待受アドレス・サーバー名・Workspace ID・公開証明書 | bridge |
| `target/dev-desktop/desktop.json` | 上記に加えて1時間・1回限りの token | Desktop |

どちらも token の扱いが違うだけで、指す Host は同じです。Host を正常終了すると自分が書いたファイルを削除します。

`Run Desktop` は `--host target/dev-desktop/desktop.json` で起動し、招待が無ければ最大15秒待ちます。一度ペアリングすると `<state>/hosts/` に profile が残り、同じ Host へは profile で再接続するため、2回目以降は使用済みの token を使いません。

`--state-directory` を指定した Host は秘密鍵と証明書を `<state>/identity` に保存して再利用するため、Host を再起動しても Desktop の profile と Web の credential は無効になりません。

`Run Desktop` と `Run Desktop (Hot Reload)` は `--state-directory target/dev-desktop/state` を共有するため、profile・設定・復元状態は Run を跨いで残ります。同じ state を同時に使うと、設定とレイアウトは後から保存した側が残り、会話は保存時に競合を検出してエラーになります。

Desktop の接続先は `--host`、環境変数 `LAPIS_HOST_FILE`、ローカル Host の自動起動の順に解決します。どれも無ければ従来どおり自分で Local Host を起動するため、製品の通常起動と Run で経路は分かれません。`--host` で Host に接続するときは Host 側の Workspace を開くため、`--workspace` は渡しません。

## Desktop

| 操作 | 手段 |
| --- | --- |
| ローカル Host を自動起動して起動 | `cargo dev` |
| `Run Host` に繋いで起動 | `Run Desktop`、または `LAPIS_HOST_FILE` を設定して `cargo dev` |
| ファイルを開いて起動 | `cargo dev --open-file README.md` |
| Hot Reload で起動 | `Run Desktop (Hot Reload)` または `cargo dev-hot-reload` |

```powershell
# Run Host に繋ぎたいとき（Run Desktop と同じ動き）
$env:LAPIS_HOST_FILE = "target/dev-desktop/desktop.json"; cargo dev
```

`cargo dev` も `Run Desktop` も Desktop と Host の両方をビルドします。2段階ビルドはありません。`Run Host` だけは Desktop を起動しないため Host だけをビルドし、Desktop のビルドが壊れていても Web 用 Host は起動できます。`Run Desktop (Hot Reload)` は Host を Cargo で、Desktop を修正版 `dx` でビルドします。

Run は Windows の Job Object で、その Run が起動した process をまとめて管理します。Stop は Run が起動した process だけを終了し、他の Run や既存の Host には影響しません。`cargo dev` や製品の通常起動では Desktop を閉じても Local Host は残るため、残した場合はログの `Desktop session:` の名前を使って `powershell -NoProfile -File scripts/stop_dev_host.ps1 -Session <session名>` で停止します。

Hot Reload は次を守ります。

- `Run Desktop (Hot Reload)` は `Run Desktop` と同じく `--host` の招待を最大15秒待ち、`target/dev-desktop/state` を共有します。違うのは Desktop を修正版 `dx` でビルドして差し替える点だけです。
- 修正版 `dx`（`target/dx-fixed0710/bin/dx.exe`）が必要です。Dioxus PR #5967 がマージされるまでの暫定です。
- 対象は Windows x64 の Debug のみで、Release には Hot Reload の runtime を含めません。
- 構造体のレイアウト変更、埋め込みアセットの変更、`request_layout` / `prepaint` / `paint` を自前実装した `Element` は反映されません。再起動してください。
- devserver への接続は起動時の1回だけです。切れたら `dx` とアプリを再起動します。
- `dx` の app 出力は worktree で共有するため、同じ worktree で同時に1つだけ起動できます（`target/hot-reload.lock` で排他）。

## Web

通常の初回接続は、Desktop が管理する Local Host を使います。別起動 Host の Web 招待管理は対象外です。

1. `clients/web` で `npm ci` と `npm run build` を実行します。Node.js 24、`wasm32-unknown-unknown`、`wasm-bindgen-cli` 0.2.129 が必要です。
2. `cargo dev` で Desktop と Local Host を起動し、設定の「Web 接続」を開きます。
3. 同一 PC または Tailscale のアドレスを選択して公開を開始し、招待リンクをコピーします。
4. 同じ PC または Tailscale 内のブラウザーで招待リンクを開きます。リンクは5分間有効で一回限りです。
5. 接続後は `/<host>/<workspace>` を使用し、同じブラウザーでは保存済み credential で再接続します。

ページと WebSocket は同じ `4747` ポートを使います。Desktop を閉じても Host と公開は継続し、Host の終了時に公開も終了します。公開中の設定は Host の再起動時に復元し、アドレスが使用できない場合は設定画面にエラーを表示します。Tailscale ACL と OS ファイアウォールで到達可能な端末を制限してください。失効は Desktop の同じ設定ページから行います。

Tailscale の候補は、Tailscale と確認できるインターフェースまたは `tailscale ip -4` の IPv4 から選びます。候補が出ない環境では Tailscale の起動と CLI の利用可否を確認してください。

公開アドレスは接続元ではなく Host の待受先です。Tailscale アドレスを選ぶと、同じ PC を含む複数の端末が同時に接続できます。招待リンクは一回限りなので、端末ごとに発行してください。`127.0.0.1` を選んだ場合は同じ PC からのみ接続できます。

Tailscale アドレスの `http://` はブラウザー上では secure context ではなく、Clipboard API や WebGPU などが利用できない場合があります。WebGPU が使えない場合は `?backend=webgl` を指定してください（例: `http://<host>/?backend=webgl#/invite?token=<token>`）。`#` より後ろに足すと token が壊れるため、必ず `#` の前のクエリ部分へ追加します。通信は Tailscale の経路で保護されますが、Web サーバー自体は TLS を提供しません。localhost はブラウザー上の扱いが異なります。

1. `Run Host` を押し、コンソールで `pair-user` を打って token を控えます。
2. `Run Web` を押すと、bridge のログに `open http://127.0.0.1:4747/` が出ます。その URL を開きます。接続先と token は URL に含まれません。
3. 初回は一覧が空なので、ログインフォームに Host URL（bridge の `ws://127.0.0.1:4757/remote`）、Workspace ID（接続情報の `workspace_id` か Host ログの `workspace=`）、手順1の pairing token を入力します。
4. 接続すると `/<host>/<workspace>` のパスで Workspace が開き、credential がブラウザーの `localStorage` に保存されます。次回は一覧から選ぶか、同じ URL を開くだけで接続できます。

`?demo=1` を付けると Host なしのサンプル Workspace で UI だけを確認できます。

Desktop からは設定 → 接続 → Web 接続で、いま使える候補（この端末 / LAN / Tailscale）を選び、ページ URL と Host URL をコピーできます。ページ URL は「ブラウザーで開く」の対象で、Host URL はログインフォームへ入力する URL です（ブラウザーで開く対象ではありません）。アドレスは保存されず表示のたびに集め直すため、Tailscale の IP が変わっても開き直せば現在の値になります。

### ポートの決め方

| 用途 | 値 | 理由 |
| --- | --- | --- |
| ページ | `4747` 固定 | ブラウザーで開く URL。バインドに失敗したら明示的に終了する |
| bridge | `4757` 固定 | ログインフォームに入力する URL |
| Host | `4787` 固定 | Desktop の保存済み profile と Workspace ID が変わらないようにする |

bridge は CLI 引数、環境変数、既定値の順に設定を解決します。`--listen` / `--origin` を省くと `LAPIS_WEB_BRIDGE_LISTEN`（既定 `127.0.0.1:4757`）と `LAPIS_WEB_BRIDGE_ORIGIN`（既定 `http://127.0.0.1:4747`）を使います。

外部公開は `--host`（ページ）と `--allow-remote`（bridge）で行います。

- `lapis-web --host` は `0.0.0.0:4747` で待ち受けます。
- `web-bridge --allow-remote` は `0.0.0.0:4757` で待ち受けます。`--listen` を併用した場合は明示したアドレスを使います。
- Host を `0.0.0.0` で起動した場合、bridge は接続情報の `0.0.0.0` をこのマシン自身として `127.0.0.1` に読み替えます。
- bridge は Origin がリクエストの Host と同じホスト名なら許可するため、IP やホスト名が変わっても同じページから接続できます。
- `--origin` を指定した場合は、その Origin も追加で許可します。

外部公開時は起動ログに接続候補が出ます。

```
open http://127.0.0.1:4747/
network http://192.168.1.20:4747/
tailscale http://100.64.1.23:4747/
```

loopback で待ち受けているときは `open` の 1 行だけです。

`4787` が他の用途で埋まっているときは `Run Host` の `--listen` を変えてください。その場合、Desktop は招待を作り直せば接続できます。

`Run Web` は別起動 Host の bridge 動作を調べる開発用構成で、事前に `npm run build` が必要です。初回は招待リンクを要求します。Desktop が管理する Local Host の招待は別起動 Host では使えないため、`Run Host` のコンソールで `pair-user` を実行し、表示された token から `#/invite?token=<token>` 形式の招待リンクを作って開いてください。製品の招待発行には `cargo dev` の Local Host を使用してください。
## Web の UI を確認する

デモ用の `Run Web (Page)`（`lapis-web --dev --dist clients/web/dist`）では変更時に Web が再ビルドされ、ブラウザーが再読み込みします。Host に接続する場合は `npm run build` を実行してから Desktop の設定で公開します。

- 反映は再読み込みで行います。Desktop の Hot Reload のように State を保ったまま差し替えるものではありません。
- 初回と依存 crate を変えたときは数十秒かかります。2回目以降は cargo の増分ビルドで数秒です。
- 250ms 以内の連続保存は1回にまとまり、ビルド中に保存した分は終わってから1回だけ追加で走ります。ビルドは常に1本です。
- 失敗すると `lapis-web` のコンソールに `web: build failed` と出力の末尾20行が出て、古い `dist` を配り続けます。画面は壊れず、修正して保存すれば再試行します。
- 監視するのは上記5ディレクトリの `.rs` と `clients/web` のページ一式です。`vendor/`（patched GPUI）や `clients/web/src/generated` 配下は監視しません。必要なら `npm run wasm` を手で実行してください。
- 再ビルドは開発サーバーだけの機能です。`npm run build` と CI は起動時に1回だけ生成します。

## Windows でのプロセス表示

Task Manager などの表示名には exe に埋め込んだ Windows リソース（`FileDescription`）が使われる。

| 表示する exe | 表示名 |
| --- | --- |
| `lapis.exe` | `Lapis` |
| `lapis-host.exe` | `Lapis Host` |
| `lapis-launcher.exe` | `Lapis Launcher` |
| `lapis-web-bridge.exe` | `Lapis Web Bridge` |

埋め込む経路は2つある。

| ビルド | 設定場所 |
| --- | --- |
| dx（Hot Reload） | `Dioxus.toml` |
| cargo（`Run Desktop`、Host、bridge） | 各 crate の `build.rs` と `assets/lapis.ico` |

dx は自前でリソースをリンクするため、`build.rs` は `DX_RUSTC` があるときは何もしない。表示名とアイコンを変えるときは `Dioxus.toml` と `build.rs` の両方を揃える。Host の表示名は静的で、インスタンスごとの名前は Task Manager では表現できない。

## 検証

| 対象 | コマンド |
| --- | --- |
| Desktop | `python scripts/check.py desktop` |
| Host | `python scripts/check.py host` |
| 全体とアーキテクチャ | `python scripts/check.py repository architecture` |
| Web のビルド | `cd clients\web; npm run build` |
| Host と bridge の結合 | `cargo build -p host -p web-bridge` → `python clients/web/wasm/tests/bridge_smoke.py` |
| launcher と Hot Reload | `python -m unittest discover -s scripts -p test_dev_session.py` |
| 更新 launcher の適用と起動 | `cargo test -p lapis-launcher` |

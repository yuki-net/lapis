# GPUI WebAssembly prototype

DesktopのUIをどこまでブラウザーで操作できるか判断するための、開発者向けプロトタイプです。
`apps/desktop/ui`のRust view・Editor・Shellをwasmへコンパイルします。
Web専用のHTMLでエディター画面を作り直してはいません。HTML側は起動・エラー表示を担当します。

## 起動

Rust 1.96以降とNode.js 24を使用します。初回はリポジトリのルートで実行します。

```powershell
npm ci --prefix apps/web
npm run setup --prefix apps/web
npm run dev --prefix apps/web
```

以後は最後の起動コマンドだけで、wasmビルド、JavaScript bindings生成、Vite起動まで行います。
ブラウザーで **http://127.0.0.1:5173/** を開きます。

- 通常はWebGPUを試し、利用できなければWebGL2へfallbackします。
- WebGL2指定: http://127.0.0.1:5173/?backend=webgl
- WebGPU指定: http://127.0.0.1:5173/?backend=webgpu （fallbackなし）
- Rustを変更したときは起動コマンドを再実行してください。Rust UIのhot reloadは実装していません。
- Clipboard APIはlocalhostまたはHTTPSなどのsecure contextと、ブラウザー側の許可が必要です。

配信用ファイルを作る場合:

```powershell
npm run build --prefix apps/web
npm run preview --prefix apps/web
```

生成物は`apps/web/dist/`です。wasmのbindingsは`src/wasm/`へ生成し、Gitには含めません。
`setup`は`wasm32-unknown-unknown`と`wasm-bindgen-cli 0.2.129`をインストールします。
Web platformはsingle-threadで起動するため、nightlyやSharedArrayBuffer用の分離headersは不要です。

## 現時点の範囲

| 操作 | Webでの扱い |
| --- | --- |
| Desktopのファイルツリー、タブ、ヘッダー、パネル | 同じRustのviewで描画 |
| テキスト入力、選択、コピー、貼り付け、Undo/Redo | 操作テスト済み。貼り付けはWebの非同期Clipboard APIへ適応 |
| ファイルを保存して閉じ、再度開く | ページ内のメモリーへ保存。操作テスト済み |
| ファイル切替、コマンド検索、パネル開閉・幅変更、ブラウザーresize | 操作と画面を確認済み |
| テーマ、Markdown previewなどの共通view | 共有実装を含む。個別の操作確認は未実施 |
| 日本語 | 描画を確認済み。実際のOS IMEによる変換操作は未検証 |
| Workspace検索 | サンプルのメモリー内ファイルを検索。backendのテストあり |
| 実ファイルの選択・保存、フォルダ作成 | OSのfilesystemへは接続しない。Openはサンプルを選び、フォルダ作成は未対応と表示 |
| PTY/Terminal、Git、LSP、外部runner | 未接続。実行要求は明示的なエラーを返す |
| 独立Inspector、複数ウィンドウ | 未対応。Webでは1ページに1つのGPUI windowを使う |

**ファイル・設定・会話状態はページ内のメモリーだけに保存します。再読み込みでリセットされます。**
Desktopの保存先や認証情報は読み込みません。入力用サンプル3ファイルを起動時に用意します。
クリップボードを使った検索はDesktopの同期APIのままなので、Webでは選択した文字列からの検索を使ってください。
ネイティブwindow操作やブラウザーが予約するショートカットも、Desktopと同一の挙動は保証しません。

## 依存と実装上の制約

| 対象 | GPUI |
| --- | --- |
| Desktop | 既存の`vendor/gpui` 0.2.2を維持 |
| wasm | `gpui-pre` / `gpui-pre-platform` / `gpui-pre-web` 0.3.9 |

`gpui-pre`はhuacnleeが公開するZed snapshotのパッケージ群です。
検証した0.3.9の元commitは`93d745668805029476e6d7fbb19652116fed51de`です。
プレビュー版はAPI変更を想定し、更新時には再検証してください。GPUI本体への追加パッチはありません。

GPUI 0.2.2とpreviewのfocus・scroll bounds・テキスト描画の差だけを`gpui_compat.rs`へ分離しました。
時刻には`web-time`、タイマーにはGPUIのexecutorを使います。
サンプルbackendの非同期処理はブラウザーのtaskへ委譲しますが、worker上での並列計算ではありません。
実際の大量ファイルやリモートbackendへの接続には、別途非同期I/Oの設計が必要です。

Webは同梱したIBM Plex SansとブラウザーのCJK fallbackを使うため、DesktopのOSフォントとは
字幅や描画が異なります。フォントのライセンスは`wasm/assets/FONT-LICENSE.txt`にあります。
今回の開発用profileではwasm本体は約17.7MB、gzip換算で約5.46MBです。
配布サイズ・初回起動の最適化や、Safari/Firefox・モバイルの互換性は未検証です。

## 検証

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p lapis-web --target wasm32-unknown-unknown --locked -- -D warnings
npm run build --prefix apps/web
npx --prefix apps/web playwright install chromium
npm run test:browser --prefix apps/web
```

ブラウザーテストはbuild済みのサイトを4173番で起動し、専用ブラウザーで操作します。
既存のブラウザーやOSのclipboardは変更しません。コピー結果と貼り付けデータはテスト内で捕捉します。
`PLAYWRIGHT_CHANNEL=msedge`でインストール済みEdgeも使用できます。
`PLAYWRIGHT_BASE_URL`を指定すると、起動済みの開発サーバーを対象にできます。
画面・失敗時のtraceは`apps/web/test-results/`に出力します。

実画面の手動確認では、ツリーからファイルを開く、文字を入力する、保存してtabを閉じて再度開く、
Ctrl+Shift+Kでコマンドを検索する、Ctrl+Jで下部パネルを開く、仕切りをdragする操作を試してください。

参考:

- [ZedのGPUI Web examples](https://github.com/zed-industries/zed/tree/56934ec0560bce8929025497411de7f5322fb6ca/crates/gpui_web/examples/hello_web)
- [使用したgpui-pre 0.3.9](https://crates.io/crates/gpui-pre/0.3.9)

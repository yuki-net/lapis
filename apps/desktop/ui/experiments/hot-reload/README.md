# Desktop UI ホットリロード検証

このディレクトリは、Windows の Subsecond クラッシュを再現し、修正後の Desktop UI 統合を追試するための検証資料です。製品の正本は `apps/desktop/ui/src/` とテストです。

旧PoC [PR #33](https://github.com/yuki-net/lapis/pull/33) の `evidence.txt` と `poc.patch` で報告された「パッチ適用後のstack overflow」を再現条件の基準にしています。

## 結論

旧PoCのクラッシュ原因は Lapis の `HotFn` 呼び出し再帰ではありません。Dioxus CLI 0.7.10 が Windows x64 の未解決関数用スタブを次の命令で生成していたことが原因です。

```text
movabs rax, address
jmp rax
```

Windows x64 の `__chkstk` は、4 KiBを超えるスタックフレームの入口でフレームサイズを `RAX` に受け取ります。スタブが `RAX` を関数アドレスで上書きしてから `__chkstk` へジャンプするため、`sub rsp, rax` が巨大なアドレス値で実行され、`0xc00000fd`（stack overflow）になります。

この原因は、Subsecond単体の小さな関数では発生せず、16 KiBのローカル配列を追加した関数で再現しました。GPUIを含まないアプリでも再現したため、GPUIの描画ライフサイクルやEntityの再生成が根本原因ではありません。

Dioxus側では、スタブをレジスタを変更しない次の形式へ置き換える修正が提案されています。

```text
jmp qword ptr [rip+0]
<8-byte address>
```

上流修正: [Dioxus PR #5967](https://github.com/DioxusLabs/dioxus/pull/5967)。この修正を含まないCLIでのWindows実行は安全な前提にしません。`dx 0.7.10`を基準にした検証では、修正済みCLIを使用してください。

Lapis本体では、CLIのスタブを修正した後に別のWindows固有クラッシュが見つかりました。`Shell::render`から呼ぶ `thread_local!` のテーマレジストリが、パッチDLL内のMSVC TLSアクセサ経由で元のEXEに存在しないアドレスを参照し、`0xc0000005`（最終的に `0xc000041d`）になっていました。これはGPUIのWindow再生成や `HotFn` の再帰ではありません。ホットリロードDebugだけテーマレジストリをプロセス内の `LazyLock<Mutex<_>>` に切り替え、パッチDLLからTLSアクセサを実行しないようにしました。通常のDebugとReleaseでは従来の `thread_local!` を維持します。

## Lapis側の統合

`lapis-desktop-ui/hot-reload` は Debug ビルドだけで有効にできる開発用featureです。Subsecondの通知は foreground executor へ渡し、既存Windowへ `refresh_windows()` を要求します。Window、DesktopRoot、Shell、Feature、Backendの正規状態は再生成しません。

現在のホットパッチ境界は次のとおりです。

- `Shell::render`、Header、Footer
- `GitFeature::render`

この境界は背景色・余白・レイアウト・Gitパネルの表示を検証するための最小構成です。Editorの独自 `Element` の `request_layout` / `prepaint` / `paint`、および補助Featureの未接続メソッドは対象外で、型レイアウトや状態構造の変更は再起動が必要です。GPUI forkは変更していません。

検証時点では上流PR #5967が未マージのため、stock CLI 0.7.10をそのまま使用しません。次のようにCLIへパッチを適用してビルドし、その `dx.exe` を使用します。

```powershell
git clone --branch v0.7.10 https://github.com/DioxusLabs/dioxus.git target/dioxus-cli-0710
Copy-Item apps/desktop/ui/experiments/hot-reload/dioxus-cli-windows-x64.patch `
  target/dioxus-cli-0710/lapis.patch
git -C target/dioxus-cli-0710 apply lapis.patch
cargo install --path target/dioxus-cli-0710/packages/cli --locked `
  --root target/dx-fixed0710
target/dx-fixed0710/bin/dx.exe serve --platform windows --hot-patch `
  --interactive false --open false --package lapis --features hot-reload
```

`dx`が上流PR #5967相当のスタブ修正を含むことを確認してから実行します。Rust UI変更を保存し、同じPID・Windowで表示が変わることを確認してください。

修正版CLIとテーマレジストリの回避を含む実機試験では、LapisのPID `76704` とWindow数 `1` を維持したまま、Shellの背景色変更を複数回適用できました。各回で `patch-applied` と `refresh windows=1` を記録し、プロセス終了はありませんでした。パッチ適用から再描画通知までの観測値は 1,293〜1,586 ms です。修正済みCLIの初回Lapisビルドは依存を含めて約162秒でした。

## 最小再現

`minimal/` はGPUIを使わず、`gpui/` は1 Window・1 Render Entityだけを使います。どちらも Subsecond 0.7.10 と Rust 1.96.0 を使用します。

```powershell
cargo run --manifest-path apps/desktop/ui/experiments/hot-reload/minimal/Cargo.toml
```

通常の小さな関数の変更は同じPIDで反映されます。`minimal/src/main.rs` のローカル配列を維持した状態で関数を保存すると、修正前CLIでは `patch-applied` の直後に stack overflow が再現します。

## 制限

- Releaseビルドには開発用依存とランタイムを含めません。
- stockのDioxus CLI 0.7.10にはWindows x64スタブのRAX破壊が残るため、上流PR #5967が取り込まれるまでは修正版CLIが必要です。
- `thread_local!` のTLS回避はホットリロードDebug専用です。別のパッチDLLからWindows TLSを参照するコードには同様の問題が残る可能性があります。
- Subsecondが対応しない構造体レイアウト変更、初期化・埋め込みアセットの変更、独自Elementの未接続処理は再起動が必要です。
- テーマJSONの監視はRustコードのホットパッチと別機構で、今回の検証対象外です。
- 実画面での未保存編集、タブ、Panel、フォーカス、Terminalの状態維持は、修正済みCLIを用いたLapis本体の追試で確認します。最小アプリの成功だけでは保証しません。

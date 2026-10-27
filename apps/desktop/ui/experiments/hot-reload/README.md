# Desktop UI ホットリロード検証

このディレクトリは、Windows の Subsecond クラッシュを再現し、修正後の Desktop UI 統合を追試するための検証資料です。製品の正本は `apps/desktop/ui/src/` とテストです。

旧PoC [PR #33](https://github.com/yuki-net/lapis/pull/33) の `evidence.txt` で報告された「パッチ適用後のstack overflow」を再現条件の基準にしています。

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

## Lapis側の統合

`lapis-desktop-ui/hot-reload` は Debug ビルドだけで有効にできる開発用featureです。Subsecondの通知は foreground executor へ渡し、既存Windowへ `refresh_windows()` を要求します。Window、DesktopRoot、Shell、Feature、Backendの正規状態は再生成しません。

### ホットパッチ境界

`Shell::render` の1箇所に `HotFn` を置き、そこから呼び出されるすべてのView / render経路を共通のホットリロード対象としています。FeatureやPanelごとに個別の `HotFn` を追加する必要はありません。

```text
Shell::render (HotFn境界)
  ├── render_header
  ├── render_main
  │     ├── Panel / Zone 内の各View
  │     └── Feature View (Git, Files, Search, Terminal, Task, Editor...)
  └── render_footer
```

### devserver 再接続

devserver が起動していない場合でもプロセスを終了せず、指数バックオフ（2秒 → 最大30秒）で非同期に再接続を試みます。UIスレッドをブロックしません。

### 起動方法

1. **通常起動**（Hot Reloadなし）: `cargo dev` または `.run/Lapis.run.xml`
2. **Hot Reload起動**: `cargo dev-hot-reload` または `.run/Lapis Hot Reload.run.xml`

Hot Reload起動時は、別途修正版 `dx` CLI で devserver を起動する必要があります。

```powershell
# devserver 起動（別ターミナル）
dx serve --platform windows --hot-patch --interactive false --open false --package lapis --features hot-reload
```

### Windows x64 patchが必要な理由

stockのDioxus CLI 0.7.10にはWindows x64スタブのRAX破壊が残っています。上流PR #5967が取り込まれるまでは、このディレクトリの `dioxus-cli-windows-x64.patch` を適用した修正版CLIが必要です。

```powershell
git clone --branch v0.7.10 https://github.com/DioxusLabs/dioxus.git target/dioxus-cli-0710
Copy-Item apps/desktop/ui/experiments/hot-reload/dioxus-cli-windows-x64.patch `
  target/dioxus-cli-0710/lapis.patch
git -C target/dioxus-cli-0710 apply lapis.patch
cargo install --path target/dioxus-cli-0710/packages/cli --locked `
  --root target/dx-fixed0710
```

upstream Dioxus が PR #5967 をマージし、修正版CLIが利用可能になったら、このpatchと修正版CLIのビルド手順は不要になります。

## 最小再現

`minimal/` はGPUIを使わず、`gpui/` は1 Window・1 Render Entityだけを使います。どちらも Subsecond 0.7.10 と Rust 1.96.0 を使用します。

```powershell
cargo run --manifest-path apps/desktop/ui/experiments/hot-reload/minimal/Cargo.toml
```

通常の小さな関数の変更は同じPIDで反映されます。`minimal/src/main.rs` のローカル配列を維持した状態で関数を保存すると、修正前CLIでは `patch-applied` の直後に stack overflow が再現します。

## 制限

- **Releaseビルド**には開発用依存とランタイムを含めません。
- stockのDioxus CLI 0.7.10にはWindows x64スタブのRAX破壊が残るため、上流PR #5967が取り込まれるまでは修正版CLIが必要です。
- Subsecondが対応しない構造体レイアウト変更、初期化・埋め込みアセットの変更、独自Elementの未接続処理は再起動が必要です。
- テーマJSONの監視はRustコードのホットパッチと別機構で、今回の検証対象外です。
- 独自 `Element` の `request_layout` / `prepaint` / `paint` は対象外です。

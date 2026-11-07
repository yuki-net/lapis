# Desktop UI ホットリロード検証

このディレクトリは、Windows の Subsecond クラッシュを再現し、修正後の Desktop UI 統合を追試するための検証資料です。製品の正本は `clients/ui/src/` とテストです。

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

`desktop-ui/hot-reload` は Debug ビルドだけで有効にできる開発用featureです。Subsecondの通知は foreground executor へ渡し、既存Windowへ `refresh_windows()` を要求します。Window、DesktopRoot、Shell、Feature、Backendの正規状態は再生成しません。

### ホットパッチ境界

GPUIの共通View dispatchに `HotFn` を置き、型付きEntityとAnyViewの通常描画を対象とします。Featureごとの登録は不要です。probeでは両方のパッチ後の描画を確認しました。独自Elementの各描画段階は対象外です。

この実装はZed / GPUIの設計を参考にしたLapis側の先行実装です。パッチ適用はSubsecondに任せ、通知後の `refresh_windows()` をGPUI foreground executorで実行します。

```text
GPUI Entity / AnyView dispatch (HotFn境界)
  └── 各ViewのRenderとそこから呼ばれるhelper
```

### devserver 接続

接続は起動時に一度だけ試みます。接続先はDioxus CLIの環境変数から取得し、UIスレッドをブロックしません。初回接続の失敗や切断後の自動再接続には対応していません。復旧するにはdevserverとLapisを手動で再起動してください。自動再接続はDioxus upstreamの改善を含む別課題です。

### 起動方法

起動手順は [開発時の起動](../../../../docs/dev.md) を参照してください。`.run/Lapis Hot Reload` または `cargo dev-hot-reload` が、Desktop と同じ launcher（`scripts/dev-session`）の `--hot-reload` を起動します。

launcher は修正版 `target/dx-fixed0710/bin/dx.exe` の存在確認と同一 worktree の排他ロック（`target/hot-reload.lock`）を取得した後、Windows x64 向けに Host をビルドしてから dx を起動します。Host のビルドに失敗した場合は dx を起動しません。修正版 dx がない場合は下記の手順で準備してください。

Host は `target/x86_64-pc-windows-msvc/hot-reload/lapis-host.exe` に置き、session へ複製して Desktop の `--local-host-executable` に渡します。dx は起動時に app 出力先を削除するため、そこへ Host を手動コピーする必要はありません。パスは worktree ルートからの相対指定なので、空白を含む worktree でも dx の引数分割に影響されません。

launcher は `hot-reload` Cargo profile を使用します。これは `dev` を継承して LTO を無効化し、workspace replay のコンパイル時間を短縮します。通常の Debug と Release には適用されず、初回起動時は専用 profile のビルドが必要です。

launcher は Windows の Job Object で Run が起動した process をまとめて管理するため、IDE の Stop で dx も終了します。強制終了した場合だけ dx が残ることがあり、その場合は残った devserver を終了してから再起動してください。

### Windows x64 patchが必要な理由

stockのDioxus CLI 0.7.10にはWindows x64スタブのRAX破壊が残っています。上流PR #5967が取り込まれるまでは、このディレクトリの `dioxus-cli-windows-x64.patch` を適用した修正版CLIが必要です。

### 既知のTLS制約

旧実装ではHot Patch後、ThemeRegistryの `thread_local!` accessorで `0xc0000005` が発生し、`0xc000041d` で終了しました。ThemeRegistryとSyntax Themeの選択値をGPUI App Globalへ移し、Lapis本体のShell / Settingsで同一PIDのパッチと状態維持を確認しました。実機検証の範囲と未確認項目は `evidence.txt` を参照してください。

独自ElementのHot Reloadは後続のPRで扱います。

```powershell
git clone --branch v0.7.10 https://github.com/DioxusLabs/dioxus.git target/dioxus-cli-0710
Copy-Item clients/ui/experiments/hot-reload/dioxus-cli-windows-x64.patch `
  target/dioxus-cli-0710/lapis.patch
git -C target/dioxus-cli-0710 apply --ignore-whitespace lapis.patch
cargo install --path target/dioxus-cli-0710/packages/cli --locked `
  --root target/dx-fixed0710
```

upstream Dioxus が PR #5967 をマージし、修正版CLIが利用可能になったら、このpatchと修正版CLIのビルド手順は不要になります。

## 最小再現

`minimal/` はGPUIを使わず、`gpui/` は1 Window・1 Render Entityだけを使います。どちらも Subsecond 0.7.10 と Rust 1.96.0 を使用します。

```powershell
$dx = (Resolve-Path target/dx-fixed0710/bin/dx.exe).Path
Push-Location clients/ui/experiments/hot-reload/minimal
& $dx serve --platform windows --hot-patch --interactive false --open false --package lapis-subsecond-probe
Pop-Location
```

最小再現では通常の小さな関数の変更が同じPIDで反映された記録があります。Lapis本体の単回接続変更後の試験結果は `evidence.txt` の `single_connection_change` と `single_connection_failure` に別途記録しています。`minimal/src/main.rs` のローカル配列を維持した状態で関数を保存すると、修正前CLIでは `patch-applied` の直後に stack overflow が再現した記録があります。

## 制限

- **Releaseビルド**には開発用依存とランタイムを含めません。
- stockのDioxus CLI 0.7.10にはWindows x64スタブのRAX破壊が残るため、上流PR #5967が取り込まれるまでは修正版CLIが必要です。
- Subsecondが対応しない構造体レイアウト変更、初期化・埋め込みアセットの変更、独自Elementの未接続処理は再起動が必要です。
- テーマJSONの監視はRustコードのホットパッチと別機構で、今回の検証対象外です。
- 独自 `Element` の `request_layout` / `prepaint` / `paint` は対象外です。

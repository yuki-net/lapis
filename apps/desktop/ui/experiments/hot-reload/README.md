# Desktop UIホットパッチ PoC検証記録（2026-10-18）

担当者が方式の再試験・採否を判断するための一時的な検証資料。製品仕様ではない。

## 結論

**PoC-A不合格。本実装には採用せず停止。** Windows上でUI crateの保存検知、再コンパイル、パッチDLL生成、Subsecond接続までは成立した。しかし背景色だけの変更で、パッチ生成直後にメインスレッドのstack overflowが2回発生した。2回目は実画面で未保存の試験テキストを入力した後に実施し、Lapisのウィンドウ消失とプロセス終了を確認した。編集状態維持は達成していない。

GPUI内部改造が不可欠とはまだ立証していない。クラッシュの根本原因（再帰、ABI、スタック使用量、パッチャ側の問題など）は未特定。Windowsで不安定な場合の停止条件に従い、GPUI改造や大規模再設計には進まない。

PoCのコードは[poc.patch](poc.patch)へ退避し、製品コードから撤去した。このDraft PRは失敗した方式の再現と次の判断のために残す。パッチは自動適用されず、通常のCargoビルドには含まれない。[evidence.txt](evidence.txt)には関連するログのみを抜粋した。

## 基準と構成

- 試験時の`origin/develop`: `4b74b57385306afdf0e25d37ff18d16bd44caeb9`。開始時の作業ツリーはクリーン。
- Rust/Cargo 1.96.0、Windows x86_64 MSVC。
- 実行crateは`apps/desktop/app`の`lapis`。`apps/desktop/ui`の`lapis-desktop-ui`へ直接依存。
- GPUIは`vendor/gpui`の0.2.2。ウィンドウ生成は`ui/src/app.rs`、ルート描画は`ui/src/shell/main/render.rs`の`impl Render for Editor`。
- Header/Main/Footerへ描画を委譲。QuickSearchは独立したRender Entity。EditorElement/SearchTextElementは独自Element。
- テーマJSONは`include_str!`、アイコンSVGは`include_bytes!`。`profile.dev.opt-level = 1`。
- CIはRust 1.96.0、Desktopはwindows-2022。
- DioxusのLatest安定版はv0.7.10（v0.8.0-alpha.3はpre-release）。Subsecond、dioxus-devtools、CLIを0.7.10で統一。
- CLIの公式Windows ZIPのSHA256を配布値と照合済み: `45eb4f87b7f86fdba8508ee7e0ef1f9e13cf8f2ffcc6a2d5858d0e8ea7dd1531`。

## 試した境界と通知

- 両crateへ明示的な`hot-reload` featureを追加。Subsecond、dioxus-devtools、async-channelはoptional。
- 実行コードは`cfg(all(feature = "hot-reload", debug_assertions))`で分離。
- 既存Editor Entityをそのまま借用して描画。Entity、Window、サービスは再生成しない。
- 最初は引数付きclosureから描画を呼び、戻り値を`AnyElement`に型消去。
- 再試験では`#[inline(never)] fn render_hot(...) -> AnyElement`という名前付き関数を`HotFn::current(Self::render_hot).call(...)`で呼んだ。
- `OnceLock`で接続・ハンドラを1回だけ初期化。`async_channel::bounded(1)`へ`try_send`し、foregroundの`App::spawn`から`App::refresh_windows()`を呼ぶ。
- GPUIの公開APIで全ウィンドウの再描画を要求。閉鎖済みWindowへの直接参照は保持しない。ただし実際の複数ウィンドウ・閉鎖通知試験は未完了。
- GPUI fork、Inspector、最適化設定、symbol mangling設定は変更していない。

## 実測

| 試験 | 結果 |
| --- | --- |
| 初回起動・closure境界 | fat build 234.32秒。PID 21984でLapis起動、devtool WebSocket接続を確認 |
| 背景色のみ保存 | `lapis_desktop_ui`変更を検知、同crateのreplayとDLL生成を確認。thin build 14,899ms、CLIのHot-patching表示3,559ms。その直後にstack overflow、終了コード`0xc000041d` |
| 名前付き境界へ変更後の再起動 | DXのfat linkで`core::hint::unreachable_unchecked::precondition_check`、`anon.*.llvm.*`、`drop_in_place`などのundefined symbolが発生。通常Cargoビルドは成功 |
| 生成物を作り直した再試験 | PoC用profileのlapis/UI生成物だけを削除し、このDXプロセスに限定して`CARGO_INCREMENTAL=0`を指定。fat build 57.71秒、PID 44632で起動 |
| 実画面と未保存内容 | 新規文書に`LAPIS_HOT_PATCH_UNSAVED_20261018`を入力し、表示を目視確認。その後、ルートの背景色だけを保存 |
| 2回目のパッチ | thin build 9,717ms、CLIのHot-patching表示1,587ms。その直後に`thread 'main' (...) has overflowed its stack`。ウィンドウ消失、終了コード`0xc000041d`を確認 |

CLIのHot-patching表示は保存から画面反映までの時間ではない。成功した画面反映は観測できず、**反映時間は測定不能**。

初回のComputer Use承認はタイムアウトしたが、ユーザー承認後の再試験ではLapisを前面にして画面取得・入力・ウィンドウ消失まで確認した。スクリーンショットは作業時の会話内で確認したが、この検証資料には同梱していない。

## 再現手順（クラッシュする実験用）

未保存の実データを使わず、通常の開発セッションと分けて試験する。

```powershell
git apply --check apps/desktop/ui/experiments/hot-reload/poc.patch
git apply apps/desktop/ui/experiments/hot-reload/poc.patch
cargo install dioxus-cli --version 0.7.10 --locked
$env:CARGO_INCREMENTAL = '0'
dx serve --platform windows --hot-patch --package lapis --features hot-reload --interactive false --open false --log-to-file target/hot-reload-retry.log
```

LapisでCtrl+Nを押し、試験文字列を入力する。`apps/desktop/ui/src/shell/main/render.rs`の`render_content`で`.bg(theme::colors().background_primary)`を`.bg(gpui::rgb(0x38104c))`へ変更・保存する。CLIの成功表示だけで合格と判断せず、実画面・PID・文書の生存を確認する。

DX再起動時のfat linkがundefined symbolになった場合、今回の再生成コマンドは次のとおり（通常Debug/Releaseの成果物は対象外）。

```powershell
cargo --config "profile.desktop-dev.inherits='dev'" clean -p lapis-desktop-ui -p lapis --profile desktop-dev --target x86_64-pc-windows-msvc
```

通常フローへの復帰はDXを終了し、背景色の試験変更を戻し、`git apply -R apps/desktop/ui/experiments/hot-reload/poc.patch`でPoCパッチだけを取り除いて`cargo run -p lapis`。上記環境変数を設定したシェルでは`Remove-Item Env:CARGO_INCREMENTAL`で解除する。他の作業差分へ`git restore`を一括適用しない。

## 検証と未検証範囲

- PoC featureあり: `cargo check -p lapis --features hot-reload`成功。
- PoC featureあり: Desktop全ターゲットのClippy（`-D warnings`）成功、テスト49件成功。
- 通常Debug: Desktop全ターゲットのcheckと`cargo build -p lapis`成功。
- 通常Release: `cargo build -p lapis --release`成功（既存Inspector操作の`window`未使用警告あり。対象外のため修正していない）。
- 通常依存グラフにSubsecond／Dioxusなし。
- PoC撤去後: 通常Desktop全ターゲットのテスト49件成功、Clippy（`-D warnings`）成功、`cargo fmt --all -- --check`成功。作業ツリーに差分なし。
- `git apply --check`で保存パッチの再適用可能性を確認。
- 基準developのCI: [run 39062308455](https://github.com/yuki-net/lapis/actions/runs/39062308455)成功。**PoC変更のGitHub CIは未実行**であり、この結果で代用しない。
- PoC-B（別ファイル・ネスト・要素追加削除・独立Entity/Element）はPoC-A失敗により未実施。
- 連続保存、構文エラーからの復帰、複数ウィンドウ、閉鎖後通知、タブ・パネル・フォーカス・スクロール維持は未検証。
- Release featureありのバイナリから開発用ランタイムが完全に除去されることは未検証。本実装へ採用していない。
- テーマJSON監視・SVG監視は未実装。既存操作の変更・削除なし。

## 既知問題との照合

- [#5314](https://github.com/DioxusLabs/dioxus/issues/5314): UI crateは直接依存であり、実際に変更検知と`replaying crates: ["lapis_desktop_ui"]`を確認。
- [#5778](https://github.com/DioxusLabs/dioxus/issues/5778): tipはbin、UIは別のlib package。空replayの典型ログとは異なる。ただしDLL内の変更関数の完全なシンボル検査はしておらず、すべて正常と断定しない。
- [#5626](https://github.com/DioxusLabs/dioxus/issues/5626): 今回は戻り値を`AnyElement`に固定し、型変更を行っていない。新コードが静かに反映されない症状とも異なる。`-Csymbol-mangling-version=v0`は適用していない。

## 次の候補

| 候補 | 判断・影響 |
| --- | --- |
| Lapis側の最小フック | GPUI変更を避けられる。まず今回のstack overflowのスタック取得とSubsecond最小再現で原因特定が必要。独立Render/Elementも検証が必要で、ルートのみの成功では採用できない |
| GPUI-CEなど対応済み実装 | Render/RenderOnce、layout/prepaint/paint、ウィンドウ通知の統合例がある。ただし公開成功例はLinux WaylandでありWindows成功の保証ではない。GPUI移行とInspector別作業への影響調査が必要。今回は移行しない |
| GPUI上流へ公開API・機能提案 | アプリ各所のフックを減らせる可能性がある。型消去された描画・Elementの呼び出しとパッチ後の無効化をフレームワーク側で扱う設計が必要。上流対応とWindows試験を要する |
| 通常再ビルドとの分担 | 型レイアウト・初期化・埋め込みアセットを再起動扱いにできる。ただし今回の背景色変更自体が失敗しており、現時点ではRust UIホットリロード達成にはならない |

参考実装: [GPUI-CE PR #292](https://github.com/gpui-ce/gpui-ce/pull/292)（2026-09-30 merge、Subsecond 0.7.10、Linuxで2ウィンドウと片側閉鎖後の更新実績）。[HotFn API](https://docs.rs/subsecond/0.7.10/subsecond/struct.HotFn.html)。

## セルフレビュー

PoCは単一ハンドラ・上限1件の通知・foreground更新で、UIスレッドへI/OやWindowの強参照を持ち込んでいない。一方、パッチ適用と描画の競合、ABI、DLLの保持、終了時の待機タスクの扱いは本番品質を検証できていない。クラッシュにより状態維持要件を満たさず、製品コードに残さない判断とした。GPUIとInspectorの差分はゼロで、他作業者の変更には触れていない。

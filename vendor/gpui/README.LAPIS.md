# Lapis vendored GPUI

保守担当者が純正依存への移行条件と残る差分を判断するための記録です。

基準はcrates.ioの`gpui 0.2.2`です。元のZed commitは
`69e2130295c2649963eb639fc70b4f2ee8ea1624`、crateのchecksumは
`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`です。

## 残す差分

Inspectorの独自変更は撤去し、`element.rs`、`inspector.rs`、`window.rs`は公式crateと同じ内容に戻しています。
改行コードを正規化して全ファイルを比較した場合、Cargoの取得管理ファイル（`.cargo-ok`等）と
追加の本READMEを除き、差分は次の2ファイルだけです。
再適用用の差分は[`patches/gpui-0.2.2-windows.patch`](../../patches/gpui-0.2.2-windows.patch)にあります。
このpatchはcontextなしです。上記checksumの公式ソースを`vendor/gpui`へ配置したうえで、リポジトリのルートから
`git apply --unidiff-zero --directory=vendor/gpui patches/gpui-0.2.2-windows.patch`で適用できます。

- `src/platform/windows/events.rs`: 移動・リサイズ中の進捗メッセージでforeground処理と描画を進め、
  `WM_MOVE`でも描画を要求し、非クライアント領域の左ボタン処理をWindowsへ渡します。
  導入時のコメントでは、モーダルループで低優先度の`WM_TIMER`が処理されず表示が遅れる問題への対策です。
  導入履歴: `f6b7fee`（ウィンドウサイズ変更処理の改良）。
- `src/platform/windows/directx_devices.rs`: Debug Layerを`LAPIS_GPUI_DX_DEBUG=1`または`true`で有効にする
  opt-inです。未設定時は無効です。導入履歴: `b442981`（開発ビルドの描画性能改善）。
  純正はDebug Interfaceが利用可能ならDebugビルドで自動有効、Releaseでは無効です。

新たなGPUI改造は追加していません。Inspectorの要素管理・Pick・状態取得は公式実装に委譲しています。

## 純正依存への切り替え検証と保留条件

2026-10-19に`gpui = "=0.2.2"`へ一時切り替えし、Inspector描画テストを含むworkspaceの
check・test・clippy・Debug/Releaseビルドを検証しました。結果は対応するDraft PRに記録します。
ただし、Windows実画面の取得ができず、移動・リサイズ中の応答性とDebug Layerによる性能差は未確認です。
既存の対策を安全に撤去できる根拠が揃っていないため、今回はWindows差分とpath依存を維持します。
純正で重大な回帰を再現したという結論ではありません。

完全撤去を判断するときは、同一条件でこのvendorと純正0.2.2を比較してください。

1. Debug/Releaseをそれぞれ起動し、Inspectorの開閉・Pick・Box Model・スタイル・幅復元を確認する。
2. 処理やアニメーションが動く状態でタイトルバーを連続ドラッグし、境界・角の連続リサイズ中も
   本文の描画、タスクの進行、終了後の入力が停滞しないか確認する。
3. Debug Interfaceを利用できるWindowsで、vendorの環境変数未設定／`LAPIS_GPUI_DX_DEBUG=1`と
   純正Debugを比較し、初期化・描画エラー・操作時のフレーム遅延を確認する。Releaseでも初期化と操作を確認する。
4. 回帰がなければworkspaceの依存を`gpui = "=0.2.2"`に変更し、vendorのexclude設定・本vendor・Windows patchを
   削除する。workspaceテスト、Windows CI、Debug/Releaseビルドを再実行する。

InspectorはLapisではDebug限定です。`inspector` featureを有効化してReleaseへ持ち込まないでください。

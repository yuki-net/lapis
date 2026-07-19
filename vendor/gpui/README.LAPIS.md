# Lapis vendored GPUI

このディレクトリは crates.io の `gpui 0.2.2` を基準とするローカルforkです。
元のZed commitは `.cargo_vcs_info.json` に記録された
`69e2130295c2649963eb639fc70b4f2ee8ea1624` です。

GitHub認証を利用できない開発環境でも同じソースをビルドできるよう、Lapisはこのcrateを
path依存で参照します。Lapis固有の変更は `src/window.rs` の外部Inspector表示APIだけです。
確認用の差分は `../../patches/gpui-0.2.2-external-inspector.patch` にあります。

- 対象ウィンドウ内へInspectorを描画しない外部表示モード
- 共有するInspector Entityの取得
- 外部Inspectorは待機状態で開き、Pick操作時だけ入力を捕捉
- 外部Inspectorウィンドウの再描画通知
- Inspectorの明示的な無効化
- 既存の同一ウィンドウInspectorとの互換性維持

Inspector APIは `inspector` featureまたはdebug assertionが有効な場合だけコンパイルされます。
Lapisはfeatureを有効化しないため、releaseビルドではInspectorコードは除外されます。

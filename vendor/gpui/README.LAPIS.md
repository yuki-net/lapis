# Lapis vendored GPUI

このディレクトリは crates.io の `gpui 0.2.2` を基準とするローカルforkです。
元のZed commitは `.cargo_vcs_info.json` に記録された
`69e2130295c2649963eb639fc70b4f2ee8ea1624` です。

GitHub認証を利用できない開発環境でも同じソースをビルドできるよう、Lapisはこのcrateを
path依存で参照します。保守担当者が更新時に残す差分と削除条件を判断するため、
公式crateとの差分を次の三系統で管理します（patchの適用先はこのディレクトリ）。

| 差分 | 対象 | 必要性・削除条件 |
| --- | --- | --- |
| `../../patches/gpui-0.2.2-external-inspector.patch` | element.rs / elements/div.rs / inspector.rs / view.rs / window.rs | 公式に外部表示、Tree取得、任意選択、Tree Hover、描画完了通知、Overlay差し替えが揃えば削除できる |
| `../../patches/gpui-0.2.2-windows.patch` | platform/windows/events.rs / directx_devices.rs | 移動・リサイズ中の描画/foreground処理と非client入力、DX Debug Layer opt-in。公式で同等動作を実機検証できるまで維持する |
| `../../patches/gpui-0.2.2-drag.patch` | app.rs / elements/div.rs / window.rs | 開始したボタンと別のMouseUpでactive dragを終了しない。公式で同等のドラッグ動作が保証されれば削除できる |

- 対象ウィンドウ内へInspectorを描画しない外部表示モード
- 共有するInspector Entityの取得
- 外部Inspectorは待機状態で開き、Pick操作時だけ入力を捕捉
- 検査対象のFrame完了をInspector Entityへ通知（Windowの保持や更新はLapisの購読側）
- Inspectorの明示的な無効化
- 既存の同一ウィンドウInspectorとの互換性維持

公式0.2.2の`set_inspector_renderer`、`register_inspector_element`、Pick/Hit Testing、
`DivInspectorState`取得を利用します。公式`toggle_inspector`は対象に30remを予約するため、
独立Inspector用には使いません。root幅の上書きだけではprompt等の内部layoutと一致しません。
Treeはsource locationを取得できる描画要素のsnapshotです。未描画の仮想要素は含みません。

GPUIは要素ID、親子/Bounds、検査状態、Pick、任意選択とHoverを扱います。
Lapisの`devtools/inspection.rs`が境界を持ち、Treeの開閉・表示行cache、色分けOverlay、
独立Windowとその購読はLapisが所有します。Overlay callbackがfalseなら公式の単色表示へ委譲します。
Inspector未起動時は要素ID/Treeを収集せず、終了時はFrameの検査用容量を解放します。
検査中はcached viewの収集省略を止め、Treeの欠落を防ぎます。通常時のcacheは変更しません。
外部Inspectorは読み取り用として現在のDiv Styleを取得し、保存したStyleを対象へ再適用しません。
Treeが同じFrameではsnapshotを複製せず、Style等を含む更新通知は描画完了時にまとめます。
ポーリングや全Windowの強制再描画は追加しません。

監査はcrates.ioの`gpui-0.2.2`と`git diff --no-index --ignore-space-at-eol`で比較します。
保存patchは`git apply --unidiff-zero --ignore-space-change --check --reverse --directory=vendor/gpui patches/<patch名>`で確認できます（Windowsの改行差を許容）。

Inspector APIは `inspector` featureまたはdebug assertionが有効な場合だけコンパイルされます。
Lapisはfeatureを有効化しないため、releaseビルドではInspectorコードは除外されます。

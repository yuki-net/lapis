# 公式GPUIと別ウィンドウInspectorの検証

Lapisの保守担当者が、fork撤去に伴う操作変更と実装方式を判断するためのPoCです。
採用済みの内部仕様ではありません。本体の依存・UI・vendorは変更しません。

## 対象と判断

- Lapis: `develop` / `4b74b57385306afdf0e25d37ff18d16bd44caeb9`
- PoC: crates.ioの`gpui = "=0.2.2"`。独立workspaceとlockfileでpath依存を排除
- crateの元commit: `69e2130295c2649963eb639fc70b4f2ee8ea1624`
- 上流main確認: `56934ec0560bce8929025497411de7f5322fb6ca`

**本体への置換は保留します。** 公式Inspectorから別ウィンドウへ詳細を表示する経路は
ありますが、検査対象側の30remの予約領域を公開APIで外せません。
空のInspector rendererでも対象のレイアウトが縮みます。要素ツリー取得とツリーからの
選択にも公開APIがなく、現在のInspectorを移植するだけでは目的を満たしません。
これはコンパイルエラーの回避では解消できない設計上の制約です。

上流mainの`inspector.rs`と`window.rs`でも、同じ非公開状態と30remの幅予約を確認しました。
この検証はmainへの依存更新やmainのビルド互換性を保証しません。

## 現行機能と依存

入口はdebugビルドのDeveloperメニューとcommand paletteです。releaseでは登録しません。
現行実装は`apps/desktop/ui/src/devtools/`にあり、既に独立ウィンドウを開きます。

| 操作・表示 | 現行の仕組み | 公式0.2.2での扱い |
| --- | --- | --- |
| 開閉・対象終了時の追従 | `InspectorController`とwindow-closed購読 | `open_window`、handle更新、終了購読は利用可 |
| 検査対象を縮めない別ウィンドウ表示 | `enable_external_inspector`、`set_external_inspector_window`、`disable_inspector` | 対応する公開APIなし |
| Pick開始、hover、クリック確定、ホイールによる深さ変更 | GPUIのInspectorとhitbox | 公開`start_picking`と標準の入力処理を利用可 |
| RefreshでPickを終了して状態更新 | 独自`stop_inspector_picking` | 明示的なPick停止APIなし。Inspector全体のtoggleは可能 |
| 要素ツリー、型名、親子関係、全ノードBounds、revision | 独自`InspectorElementNode`とframe収集 | 任意の描画済みツリーを列挙する公開APIなし |
| ツリー開閉、選択に合わせた祖先展開、消滅ノードの整理 | Inspectorへ追加した状態 | UI側へ移せるが、入力となるツリーが必要 |
| ツリークリックによる選択と詳細更新 | 独自`select_inspector_element` | 任意IDを選択する公開APIなし |
| ツリーhoverによる対象プレビュー | 独自`preview_inspector_element`とhover状態 | 対応する公開APIなし |
| 選択要素のID・生成元 | `InspectorElementId.path` | `active_element_id`から利用可 |
| DivのBounds・content size・Box Model・Style・JSON | `DivInspectorState`と登録renderer | 公開`register_inspector_element`と`render_inspector_states`で再利用可 |
| 対象上のmargin/border/padding/contentの色分け | 独自`paint_inspector_hitbox` | 標準はhitboxの単色表示。色分けの維持にはLapis側の描画が必要 |
| 別ウィンドウへの自動再描画 | 独自`refresh_external_inspector` | 公開rendererからdeferし、Lapis側で通知する経路は構成可 |

現在の詳細表示は読み取り用です。Style編集機能を実装済みとみなしていません。
「全要素ツリー」もGPUIがsource locationを取得できるinspectable要素のsnapshotであり、
ブラウザーDOMのような永続ツリーではありません。

## vendorの実差分

公式crateと改行差を除いて比較したところ、変更は次の5ファイルにあります。

- `src/element.rs`: 要素型・親子関係・Boundsの収集フック
- `src/inspector.rs`: ツリー、hover状態、状態取得、選択表示制御
- `src/window.rs`: 外部Inspector、frame公開、選択・preview、色分け描画、通知
- `src/platform/windows/directx_devices.rs`: debug layerを`LAPIS_GPUI_DX_DEBUG`のopt-inに変更
- `src/platform/windows/events.rs`: 移動・リサイズ中の描画とtask処理、非client領域のマウス処理

`vendor/gpui/README.LAPIS.md`はwindow.rsのみの変更と説明していますが、実態と一致しません。
`patches/gpui-0.2.2-external-inspector.patch`にも後続の変更が含まれていません。
Inspectorを分離できても、最後の2件についてWindowsの操作性・描画性能を検証せず
vendor全体を撤去すべきではありません。今回それらの実装は変更しません。

比較例（`$official`はCargo registryの`gpui-0.2.2`ディレクトリ）:

```powershell
git diff --ignore-space-at-eol --no-index -- $official vendor/gpui
```

## PoCの責務

`InspectorSession`は対象window handleと弱参照のInspectorだけを保持します。
`DevTools`は独立したwindowのrootで、検査対象のInspectorから詳細を描画します。
対象側のInspector rendererはEntityの弱参照をdeferで渡し、DevToolsへ通知します。
対象の描画中に他のwindowへ再入せず、破棄されたsession/windowへの更新はエラーとして扱います。

Console・Network追加時にはDevToolsの表示状態とpanelを追加し、それぞれが必要な
データを所有する構成にできます。InspectorSessionへログや通信状態を集約する必要はありません。
このPoCには空のConsole/Network、plugin registry、汎用trait、hot reloadを実装しません。

## 操作変更と代替案

以下は方式選択の比較であり、本体に実施した変更ではありません。コストは相対評価です。

| 案 | 維持できる操作・新たな構成 | 失う操作・制約 | コスト・判断 |
| --- | --- | --- | --- |
| A: 公式の埋め込みInspector | Pick・深さ変更・Div詳細。別のDevTools windowに将来のConsole等を配置可 | Inspector自体は対象内。ツリー、ツリーhover/選択、色分けは失う | 小。独立Inspectorという目的には不一致 |
| B: 本PoCの別window表示 | Pick・Div詳細・生成元を別windowへ表示。DevTools表示をInspectorから分離可 | 対象の30remが残る。ツリー関連操作なし。開くと直ちにPick開始 | 小〜中。独立表示はできるが、移行先には推奨しない |
| C: Lapis側で対象を明示的に計装 | 通常の独立DevTools window、意味のあるcomponent選択、独自overlay等を設計可 | 未計装のGPUI内部要素は検査できない。任意の全要素を透明に列挙する仕組みにはならない | 大。wrapper/Elementによる収集、座標・clip・順序・ID・世代管理の別PoCが必要 |
| D: 必要なAPIを上流へ提案 | 採用されれば公式依存で外部表示とツリーを実現できる可能性 | 採否・公開時期・APIは未確定 | 中＋外部依存。完全な要素検査が必要なら優先して検討 |
| E: 現forkを一時維持し責務だけ分離 | 現行操作を維持してDevTools rootとInspector取得層を分離可 | forkの追従負担とWindows patchの保守は残る | 中。暫定策でありfork解消の完了にはならない |

現行比で新しく実証した利用者操作はありません。既に別ウィンドウで使えるためです。
PoCで確認するのは「公式API経由でも詳細表示の責務を別windowへ出せるか」であり、
Console・Networkが利用可能になったという成果ではありません。

独立windowと公式依存を優先し、検査範囲をLapisのcomponentへ限定できるならCが候補です。
任意のGPUI要素を検査する必要があるならD、当面の操作維持が必要ならEを選びます。
この範囲判断なしにCを本実装すると、計装コストと失われる検査範囲を過小評価します。

## 再現と検証

ルートから実行します。Lapis本体のworkspaceとは別に実行する必要があります。

```powershell
cargo fmt --manifest-path tools/gpui-devtools-poc/Cargo.toml -- --check
cargo check --manifest-path tools/gpui-devtools-poc/Cargo.toml --locked --all-targets
cargo test --manifest-path tools/gpui-devtools-poc/Cargo.toml --locked --all-targets
cargo clippy --manifest-path tools/gpui-devtools-poc/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path tools/gpui-devtools-poc/Cargo.toml --locked --all-targets
cargo run --manifest-path tools/gpui-devtools-poc/Cargo.toml --locked
```

1. TargetとDevToolsの2つのwindowを並べる。
2. Target側に30remの予約領域があり、通常UIの幅が縮むことを確認する。
3. 青枠のDivをhover・クリックし、DevToolsにID・生成元・Bounds・Styleが出ることを確認する。
4. DevToolsのPick elementを押し、Target上でホイールによる深さ変更と再選択を確認する。
5. どちらかのwindowを閉じる。PoCは両方を終了する。本体用の再開・複数対象管理は未実装。

自動テストは、空rendererでも30rem縮むこと、toggle解除による幅復元、PickしたDivの
Boundsを別windowのrendererへ渡せること、Pick再開、閉じたwindow handleへの更新失敗を確認します。
Windows CIのDesktop jobにもPoCのformat/check/test/clippy/buildを追加しています。
実画面確認の結果と実行済みチェックの結果はPRに記録します。

## 根拠

- [公式0.2.2 Inspector API](https://docs.rs/gpui/0.2.2/gpui/struct.Inspector.html)
- [公式0.2.2 Window API](https://docs.rs/gpui/0.2.2/gpui/struct.Window.html)
- [crate元commitのwindow.rs](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/src/window.rs)
- [確認した上流mainのinspector.rs](https://github.com/zed-industries/zed/blob/56934ec0560bce8929025497411de7f5322fb6ca/crates/gpui/src/inspector.rs)
- [確認した上流mainのwindow.rs](https://github.com/zed-industries/zed/blob/56934ec0560bce8929025497411de7f5322fb6ca/crates/gpui/src/window.rs)

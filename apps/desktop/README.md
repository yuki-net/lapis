# Desktop

Rust/GPUIによるDesktop Clientの実装領域です。現在のファイル配置やEntity分割はプロトタイプであり、今後の設計変更を妨げる規則ではありません。

## 役割と理由

- Window、Panel、focus、layoutを扱うShellと、個別の機能の状態を混同しない。全機能の可変状態を巨大なShellへ集約しないためです。
- 描画は現在の状態からUI要素を作ることに集中し、Render中の外部I/OやUIスレッドのblockingを避ける。描画の応答性を守るためです。
- Editorの入力や選択など即時性が必要な表示状態を、不必要な外部応答待ちにしない。
- 非同期の結果は、対象の状態や世代を確かめてから適用する。古い結果による画面の巻き戻りを防ぐためです。
- GPUI固有の型やEventをバックエンドの共通契約へ漏らさない。

必要な境界は状態の所有者やライフサイクルで判断し、行数のためだけのEntity・ファイル分割をしません。

UIの共通の意図は [Design](../../docs/DESIGN.md)、手動検証の扱いは [AGENTS.md](../../AGENTS.md) を参照してください。

## 開発用Inspector

DebugビルドのHelp → Toggle Inspector、またはコマンド検索の`dev: Toggle Inspector`で開閉します。
F12はEditorのGo to Definition用です。Inspectorには割り当てていません。

GPUI公式のInspectorが対象ウィンドウの右側30remを使用し、閉じると通常画面の幅に戻ります。
Pick elementで対象をクリックして選択し、ホイールで重なった要素の深さを変更できます。
Pick中は公式GPUIがマウス入力を捕捉するため、対象を選択してからCloseなどを操作します。
DivのBox Model、content size、StyleRefinement、生成元を縦方向に表示します。
選択・検査状態と対象Windowの連携はGPUIが所有し、Lapis DevToolsは表示の登録だけを担当します。
独立OSウィンドウ、全要素ツリー、ツリーの折り畳み・hover・任意選択、外部Windowへの同期はありません。
Inspectorは通常のPanelHostには属さず、ReleaseにはDevToolsモジュールを含めません。

画面確認では開閉、Pickと再Pick、Box Model・スタイルのスクロール、リサイズ後の配置、
Close後の通常UI復元、複数Windowを開いた状態での個別の開閉を確認してください。

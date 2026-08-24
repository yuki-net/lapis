# DESIGN.md

Lapisの視覚的一貫性、画面構造、共通UIの振る舞いを定義します。具体的な色値、比率、型、ファイル構成は各ClientのDesign Tokenと実装を正とします。

## Status

この文書はPhase 1のDraftです。確定済みの節は実装規則として扱い、`未確定事項`は利用者との確認が終わるまで既存UIや他製品から推測して補完しません。未確定事項が残っていること自体を、確定済みの設計を実装しない理由にはしません。

## デザイン方針

- JetBrains Fleetの軽さと骨格を基礎に、IntelliJ IDEAの操作性と情報密度を取り入れる。
- AI機能では、会話一覧、編集・会話、Filesを行き来しやすいAI editor型の構造を目指す。
- 装飾より情報と操作を優先し、強い影、gradient、glass表現を常用しない。
- Desktopは情報をcompactにし、Mobileは文章の密度を保ちながら操作領域をtouch向けに広げる。
- 未実装機能を利用可能に見せない。一時的に利用できない機能とは表示上も区別する。

## Flat Island

通常画面は、平面的な構成の中でSurfaceの明度差、余白、薄い境界によって領域を分けます。

- Panelは背景から一段だけ浮いたIslandとして扱う。
- 通常のPanelに影を付けない。
- Menu、Popover、Dialogには必要な場合だけ弱い影を許可する。
- 基本の角丸は`6px`とし、円形またはpillだけ`full`を使う。
- 連結面、分割線、画面端では角丸なしを許可する。
- 現在の暗い背景と青いaccentを基本とする。
- accentは選択、focus、主要操作、接続状態など意味のある箇所に限定する。

## Design Token

色、余白、角丸、文字役割、motion、Panel比率は、Viewへ数値を直接書かずsemantic tokenを経由します。

- DesktopはRust、MobileはKotlinでnativeに定義する。
- 両Clientは同じsemantic token名と意味を共有する。
- Rust実装をKMPへFFI公開しない。
- 画面構造をJSONまたはXMLで宣言しない。
- 文書には値の一覧を重複させず、使い分けと不変条件だけを残す。

IconはLucideを基本とし、各ViewからSVG pathを直接参照しません。LapisのIcon IDとIcon componentを介し、将来のIcon theme差し替えを可能にします。

## Desktop Shell

Desktopは次の4領域を同じPanelモデルで扱います。

- Left Panel
- Main Panel
- Right Panel
- Bottom Panel

Main Panelは常時表示し、他のPanelは利用者の操作だけで開閉します。画面幅を理由にPanelを自動で閉じません。狭い場合は、表示中Panelの比率を保ちながら内容の省略、icon化、scrollで対応します。

### Panel

PanelはHeader、Tab、Body、空状態、drag/drop領域を共通化します。Panel固有の機能ロジックを共通Frameへ入れません。

同じ軸のscroll所有者は一つに限定します。Panel標準を使わないFeatureは、Feature固有のscroll所有権を登録契約で明示します。

DocumentとToolは完全に同じTabとして扱います。

- Tabの形状と操作を区別しない。
- iconで内容を判別する。
- dirty markerをTabへ表示しない。
- close iconは常時表示する。
- TabはPanel間をdragして移動できる。
- Tabが収まらない場合は横scrollし、active Tabを表示範囲へ移動する。
- Main PanelにもDocumentとToolを配置できる。

未保存状態とRevisionは表示の有無にかかわらず維持します。未保存Documentを閉じるときは、保存、破棄、cancelを確認します。アプリ終了時は未保存Documentを一つのDialogへまとめます。

### Panel配置

Panelサイズは固定pxではなく利用可能領域に対する比率として扱い、Conversationごとに保存します。Mainを含む表示中Panelを限界まで圧縮しても、自動では閉じません。

Bottom PanelとSide Panelの優先関係は左右独立です。

- 通常はLeftまたはRightが全高を使用し、Bottomはその内側まで表示する。
- Bottomの左境界を外側へdragすると、BottomだけがLeftの下まで広がり、Leftは上へ縮む。
- Bottomの右境界も同じ規則で独立して切り替える。
- 逆方向へdragすると元の配置へ戻す。
- 左右の優先状態、Panel比率、Tab、active Tab、開閉状態をConversationごとに保存する。

### 空状態

空のPanelにはPanel名とToolを開く操作を表示します。Main Panelが空の場合は中央に`Open...`を表示し、次を選択できます。

- Open Folder: 選択したdirectory全体をWorkspaceとして開く。
- Open File: 選択した単一fileだけを対象として開く。

単一fileの親directoryが既存Workspaceとして開かれている場合は、そのWindowへ移動してfileを開きます。同じfileがすでに開かれている場合も既存WindowとTabへ移動します。

## WindowとConversation

WindowとConversationは別の概念です。

- 一つのDesktop Windowは、一度に一つのWorkspaceを表示する。
- 同じWorkspaceを複数のDesktop Windowで重複して開かない。
- 既に開いているWorkspaceを指定した場合は、そのWindowへ移動する。
- 別Workspaceを開く場合は`This Window`、`New Window`、`Cancel`を選択でき、既定値を保存できる。
- Conversationを切り替えてもWindowを増やさない。
- Windowごとに表示中Workspaceと選択中Conversationを保持する。
- 終了時に開いていたWindowを復元する。

`Close Window`は表示を閉じ、`Close Workspace`はWindowを空状態へ戻します。どちらもProjectやConversationを削除しません。Conversationの削除は別の明示操作とします。

ConversationはProjectとWorkspaceに所属します。Conversation本文、Task、Executionはbackendが保存し、表示中ConversationとPanelなどのView StateはClientごとに保持します。DesktopとMobileは同じWorkspaceで別のConversationを表示できます。

新しいConversationは、現在のPanel配置、比率、Tab、開いているDocumentを初期値として複製し、その後は独立します。実行中Terminal processは複製しません。

## Mobile

MobileはDiscord型のnavigationを基礎とし、Desktopの多Panel構造をPhoneへ縮小移植しません。融通の多さより、目的の情報へ少ない操作で移動できることを優先します。

### Phone

Phoneは一画面ずつ表示します。

- 一覧画面でSide Navigationと内容一覧を表示する。
- 項目を選択すると、詳細画面が右から全画面で遷移する。
- 詳細画面ではSide Navigationと下部領域を隠す。
- 左上の戻る操作とOSの戻る操作は同じ履歴を使う。
- 戻った一覧は選択とscroll位置を復元する。
- 利用可能な横幅が十分な場合はTablet layoutへ移行する。

Side Navigationの対象はWorkspace、Files、Terminal、Connection、Settingsを基本とします。Terminalは独立した詳細画面として表示します。

一覧画面の下部には、将来のAI入力領域を示す横長の枠とUser iconを配置します。Phase 1のAI枠はfocus、hover、clickを持たない非操作要素とし、詳細画面では表示しません。User iconはSettingsを開きます。

### Tablet

Tabletは利用可能な横幅に応じて、次の3領域を同時表示します。

```text
Navigation | List / Tool | Main Content
```

- EditorとTerminalはMain Contentへ表示する。
- 横幅が不足した場合は一覧を隠し、Phone型の一画面navigationへ移行する。
- Desktopと同じ情報を扱えても、Desktop固有の自由な4 Panel操作は要求しない。

## ThemeとMotion

Themeの内部表現はDarkとLightの二つとし、利用者の選択肢はSystem、Dark、Lightとします。

- 初期値はDark。
- SystemはOS themeの変更へ再起動なしで追従する。
- Themeはsemantic color tokenを切り替え、View固有の色分岐を増やさない。
- animationはPanel開閉、Popover、画面遷移など理解を助ける箇所に限定する。
- OSのReduce Motionが有効な場合は、移動を伴うanimationを即時切替へ置き換える。

## 共通状態

Phase 1の対象画面は、必要に応じて次の状態を持ちます。

- normal
- loading
- empty
- error
- disabled
- disconnected

未実装機能は表示しません。接続中、権限不足、処理中など一時的に利用できない操作は、必要な理由と回復操作を伴うdisabled状態として扱います。

## 未確定事項

次は実装前に追加確認が必要です。未確定事項を既存UIから推測して固定しません。

- Header中央へ表示するProject、Workspace、file、Conversationの組み合わせ。
- FooterまたはStatus Barの有無と表示項目。
- SettingsをMain限定にするか、他Panelへ移動可能にするか。
- Settingsをcategory型にするか単一一覧にするか。
- UI、Editor、Terminalの既定fontとfallback順序。
- loading、error、toast、confirmationの詳細表現と配置。
- Keyboard focus ringとMenuのkeyboard操作規則。
- Mobile接続画面の自動検出、手動入力、pairing、再接続の構成。
- Phone一覧画面におけるNavigationとListの正確な幅と開閉方法。
- ProjectまたはWorkspaceを閉じた後のRecent表示。
- 単一file用Workspaceから別の単一fileを開く場合のWindow選択。

Remoteの信頼境界、認証、暗号化、version、capability、再接続は`REMOTE.md`で定義します。具体的なmessage、型、token値はコードまたはSchemaを正とします。

# ARCHITECTURE.md

## Panel layout

- `left-panel`、`center-panel`、`bottom-panel`、`right-panel` は同じ Panel の概念として扱う。
- Tool は `ViewId` と Panel 位置を分けて保持する。拡張機能は既定位置と許可位置を宣言し、利用者の配置は Conversation の画面状態に保存する。
- `center-panel` は Document タブを表示する領域であり、Document がない場合は起動・選択画面を表示する。Tool panel と同じレイアウト状態の原則を共有するが、Document の表示と編集は UI の専用責務とする。
- アプリ起動時に以前の Project を無条件で開かない。Project を開いた時だけ、同じ Workspace root を持つ保存済み Conversation を復元する。単体ファイルは Workspace を開かず `center-panel` に表示する。

## Localization and global settings

`lapis-localization` owns `LocaleId`, `MessageId`, language-pack manifests, package validation,
and resolving `(LocaleId, MessageId)` to a display string. It does not depend on GPUI, persistence,
networking, or a language-pack store.

`lapis-settings` owns `GlobalSettings` and the settings repository contract. The selected locale is
stored in `GlobalSettings.locale`; `app-services::SettingsSession` is the application boundary that
persists changes. UI code must not access persistence directly.

`desktop-ui` renders strings returned by the Localizer only. The source of a language pack (bundled,
locally installed, or a future store) is intentionally outside the localizer and uses one package format.

## 概要

本アプリはRust製のデスクトップコードエディターである。

全体は機能単位のcrateに分ける。
DDDの4層をトップレベルには置かない。
必要な場合だけ、各crateの内部でDomain、Application、Infrastructureを分ける。

## 基本方針

- 機能単位でcrateを分ける。
- crate間の循環依存を作らない。
- UIに業務ロジックを書かない。
- 外部ライブラリの型を他のcrateへ漏らさない。
- 具体的な実装は`app`で組み合わせる。
- 共通化は必要になってから行う。

## 中心モデル

中心モデルは次の意味で使う。ID は外部表現や採番方式を固定しない不透明な値とし、
同じ種類の ID だけを比較できる型にする。

| モデル | 責務 | 所有関係とライフサイクル |
| --- | --- | --- |
| `Project` | ルート候補、プロジェクト設定、実行・ビルド設定を表す論理単位 | 0 個以上の `Workspace` から参照される。フォルダーを閉じても履歴として残せる |
| `Workspace` | ファイル、Git、LSP、Terminal、CLI を提供するバックエンド上の作業空間 | 1 つの `Project` に属する。shared、worktree、remote、container の能力を組み合わせ、open / disconnected / closed を持つ |
| `Conversation` | 人間と AI の作業文脈、および復元する画面状態の単位 | 1 つの active `Workspace` を参照し、複数の `Task` と文書表示状態を所有する。切替やUI終了で破棄しない |
| `Task` | ユーザーが依頼した論理的な仕事 | 1 つの `Conversation` に属し、再試行を含む複数の `Execution` を所有する |
| `Execution` | Task を特定の runner、Workspace、権限で実行した 1 回の試行 | 1 つの `Task` に属する。プロセス終了後も状態、イベント、worktree 関連を履歴として保持する |

`Project` と `Workspace` はディレクトリそのものではない。`Workspace` はバックエンドと
能力の組み合わせであり、同じ `Project` に shared Workspace と Task 用 worktree Workspace
が同時に存在できる。`Conversation` は画面タブではなく復元単位、`Task` は依頼、
`Execution` は実行試行である。

## Document、Revision、編集Transaction

- `Document` は `DocumentId`、Workspace 内の任意の path、Encoding、Text Buffer、現在の
  `Revision`、保存済み Revision、外部ファイル fingerprint を持つ。
- `Revision` は Document ごとに単調増加し、内容が変わる Transaction の確定時だけ進む。
  LSP 応答、保存、外部変更、リモート同期は対象 Revision を検証する。
- 編集 `Transaction` は 1 回のユーザー操作または明示的にまとめた複数 Edit の原子的な
  単位で、Undo / Redo は Transaction 単位で行う。
- 選択、キャレット、スクロールは文書内容ではなく Conversation の view state として持つ。
- 保存は期待する保存済み Revision と外部 fingerprint を渡す compare-and-write とし、
  一致しない場合は競合を返して暗黙に上書きしない。

### 編集バッファの選定

フェーズ1では `ropey 1.6.1` を `text` crate 内部だけで利用する。Ropey は UTF-8 の rope を
char index で編集し、行と UTF-16 位置の変換に必要な索引を提供する。現行の `String` は
中間編集で後続全体を移動し、大規模文書の編集や安価な部分参照に不向きである。

- 採用理由: 大規模文書で毎回全文コピーせず、UTF-8 境界を壊さず、成熟した実装を利用できる。
- 代替案: `String` 継続は性能要件を満たさない。独自 gap buffer / rope は保守・検証負担が大きい。
- 影響: MIT ライセンスの小さな純Rust依存が1つ増える。外部型は `text` crate の公開APIへ漏らさない。
- 再評価条件: 実測でスナップショット共有や編集遅延の要件を満たさない場合。

## Task と Execution の状態遷移

```text
queued ── start ──→ running
                     ├── input request ──→ waiting_for_input ── reply ──→ running
                     ├── approval request → waiting_for_approval → decision → running
                     ├── success ─────────→ succeeded
                     ├── error ───────────→ failed
                     └── cancel ──────────→ cancelled

queued / waiting_for_input / waiting_for_approval ── cancel ──→ cancelled
```

終端状態は `succeeded`、`failed`、`cancelled` とする。Task の表示状態は最新 Execution から
導出し、Execution 履歴は上書きしない。外部 CLI 固有イベントは runner adapter 内で
共通 `TaskEvent` へ変換する。

## UI とバックエンドの境界

UI は view state と低遅延な編集状態を保持するが、ファイル、Git、LSP、Task、Terminal を
直接操作しない。`desktop-ui` は `app-services` が公開する Command / Query / Event の契約だけを
利用する。ローカル実装とリモート実装は同じ契約を実装する。

```text
desktop-ui
  ├── ローカル入力、選択、スクロール、描画（バックエンド応答を待たない）
  └── Command / Query / Event
             ↓
        app-services
             ↓
  WorkspaceBackend（local / loopback / remote）
             ↓
 file / git / lsp / terminal / task-runner
```

境界を越える値は Lapis の型に変換し、外部 CLI、Git、LSP、PTY、通信ライブラリ固有の型を
UIへ渡さない。イベントには Workspace / Conversation / Document / Task / Execution の ID と、
順序または Revision を含め、古い応答を破棄できるようにする。

## 機能階層と拡張境界

「標準搭載」と「常駐」を同じ意味にしない。Lapis の機能を `Core`、`Bundled Feature`、
`External Extension` の三層に分ける。

| 層 | ライフサイクル | 対象 |
| --- | --- | --- |
| `Core` | 常に利用可能。無効化しない | Window、Shell、入力、Text Buffer、Document、Workspace、Conversation、command・settings・extension registry、fallback theme |
| `Bundled Feature` | アプリに同梱するが、activation まで重い処理を起動しない | Files、Search、Git、Terminal、Problems、Markdown、Rust、Codex、Preview |
| `External Extension` | 明示的に追加・削除でき、宣言した能力だけを利用する | 追加言語・LSP・formatter、theme、icon、locale、keymap、AI runner、remote adapter |

すべての拡張を無効にした状態でも、Core だけでプレーンテキストの表示、編集、保存、
Workspace と Conversation の復元が成立することを不変条件とする。標準機能は可能な範囲で
外部拡張と同じ registry を利用するが、Editor canvas、IME、focus、Shell のように性能と
ライフサイクルを支配する処理は Core に残す。

### Activation

機能は `installed`、`loaded`、`running` を別状態として持つ。初期の activation 条件は次を
想定するが、文字列や特定Featureの固定分岐ではなく型付き条件として扱う。

```text
OnWorkspaceOpen
OnLanguage(language_id)
OnCommand(command_id)
OnView(view_id)
OnWorkspaceCapability(capability)
```

- Files は Workspace を開いたときに読み込む。
- Search は検索コマンド実行時だけ処理を起動する。
- Git は Git view を表示したときに status 監視を開始し、閉じたら休止する。
- Terminal、Codex はユーザーの開始操作まで process を起動しない。
- LSP は対応する言語の Document を開いたときだけ起動し、対象がなくなれば停止できる。

### 安定IDとデータ拡張

表示文字列やUnicode記号を機能の識別子として使わない。少なくとも次の不透明な安定IDを
導入し、表示は theme・locale・keymap から解決する。

```text
CommandId
FeatureId
ViewId
LanguageId
IconId
MessageId
ThemeId
LocaleId
```

Core は必ず描画できる fallback theme と fallback icon を持つ。theme、icon、locale、keymap、
言語メタデータは任意コードを必要としないデータ拡張から先に対応する。command ID、設定key、
theme token、message ID は翻訳しない。ラベル、説明、状態文言だけをlocaleで解決する。

### UI Contribution

初期の拡張 UI は任意のGPUIコードを読み込まず、次の定義済みslotへの宣言的contributionに
限定する。

```text
CommandPalette
ToolDock
SideDock
BottomDock
StatusBar
EditorDecoration
SettingsPage
```

配置、focus、close、resize、theme、アクセシビリティ、低遅延入力はShellが所有する。Featureは
`ViewId`、表示slot、`CommandId`、`IconId`、activation条件、必要なWorkspace capabilityを登録する。
UI からファイル、Git、LSP、Terminal、Taskを直接操作しない既存境界は拡張にも適用する。

外部拡張のpackage形式、WASMまたは別processなどの実行方式、IPC、sandbox、署名、配布storeは
未確定とする。これらを決める前でも、Bundled Feature registryとデータ拡張を実装できる境界を
優先する。

### 汎用検索ページと開発用Inspector

右ペインの汎用検索ページはShellが配置、focus、入力を所有し、検索対象はProviderから受け取る。
初期Providerは `FeatureRegistry` の `CommandPalette` contributionを、localeで解決した表示名、
安定した `CommandId`、keymapのショートカットとともに返す。ファイル、設定、Taskなどを追加する際も
入力UIへ個別機能を直接結合せず、新しいProviderを追加する。検索入力、候補選択、Shift二度押し判定は
ローカルUI状態で完結し、backend応答に依存しない。

開発用Inspectorはdebugビルドだけで登録する `lapis.command.dev.toggle-inspector` から起動する。
本体ウィンドウはPickingと選択要素の計測、独立したInspectorウィンドウは検査結果の表示を担当し、
ライフサイクルは `InspectorController` が調停する。Inspectorを開いても本体のviewportとレイアウトを
変更しない。表示対象はGPUIの生成元、`GlobalElementId`、実bounds、`DivInspectorState` の
`StyleRefinement`であり、存在しないCSSセレクタやカスケードを模倣しない。

GPUI 0.2.2の標準Inspectorは同一ウィンドウ内へ30remの領域を予約するため、現行の公開APIだけでは
独立ウィンドウ要件を満たせない。外部表示モード、Inspector Entityの参照、既存Picking処理の再利用に
必要な最小APIだけを `vendor/gpui` のローカルforkへ追加する。GitHub認証を利用できない開発環境でも
再現可能にするため、現時点ではremote forkではなくpath依存を採用し、GPUI 0.2.2の元ソースと差分を
リポジトリ内で固定する。既存の `Window::toggle_inspector` と同一ウィンドウ表示は壊さない。
将来upstreamが同等APIを提供した時点でcrates.io版へ戻せるよう、Lapis側のGPUI固有処理は
`desktop-ui/devtools` 内へ閉じる。Inspectorの登録とControllerはdebugビルド限定とし、releaseでは
Inspector関連コードをコンパイルしない。debugでもInspectorが閉じている間はPicking、計測、外部windowの
再描画を行わない。外部Inspectorは待機状態で開き、利用者がPickボタンを押した期間だけ本体のhitbox登録と
入力捕捉を有効にする。

外部Inspectorを開いている間だけ、GPUIの`request_layout`再帰からinspect可能なElementの親子関係を
収集し、`prepaint`で確定したboundsを加えたフレーム単位のスナップショットを保持する。本体が再描画
されるたびにツリーを自動更新し、手動Refreshでも本体の再描画を要求できる。これは永続DOMではなく、
条件分岐などで消えたElementは次のスナップショットから除去する。Inspectorを閉じた状態とreleaseでは
親子関係の収集を行わない。

### desktop-ui の目標module構成

`panels`と`features`のような重複する分類軸は併用しない。Shellは配置、Featureは機能、
Componentは再利用可能な描画部品を所有する。

```text
desktop-ui/src/
├── app.rs
├── shell/
│   ├── mod.rs
│   ├── state.rs
│   ├── title_bar.rs
│   ├── tool_dock.rs
│   ├── side_dock.rs
│   └── bottom_dock.rs
├── extension_ui/
│   ├── registry.rs
│   ├── contribution.rs
│   └── activation.rs
├── features/
│   ├── editor/
│   ├── files/
│   ├── search/
│   ├── git/
│   ├── tasks/
│   ├── terminal/
│   ├── problems/
│   ├── preview/
│   └── conversation/
├── components/
├── icons/
├── keymap/
├── localization/
└── theme/
```

移行は、安定ID、共通Component、Editor canvas、Shell、Feature描画、Feature固有state、activationの
順で行う。単に関数を別ファイルへ移すだけでなく、巨大なRoot Entityから独立ライフサイクルを
持つFeature stateを段階的に分離する。

## 実装フェーズと受け入れ条件

| フェーズ | 完了条件 |
| --- | --- |
| 0 調査と設計整合 | 中心モデル、依存方向、状態遷移、境界、未確定事項、1〜5の条件が文書と一致する |
| 1 ローカル編集 | フォルダーを開き3文書以上をタブ編集でき、Unicode、Undo/Redo、検索、縦横スクロール、安全な保存、外部変更・競合検知、再起動復元が動く |
| 2 AI Task | UIからCodex Taskを開始し、共通イベント、入力・承認待ち、取消、2並列実行、再起動後の状態・ログ復元を確認できる |
| 3 Gitレビュー | Taskごとのworktreeで実行し、status/diffを確認して選択取込または破棄でき、競合を明示する |
| 4 開発支援 | rust-analyzerの診断・補完・定義移動、非同期全文検索、PTY入出力・resize・終了、不要プロセス停止を確認できる |
| 5 復元とリモート | 2 Conversationの全画面状態、異常終了復元、loopbackまたは実環境で切断・再接続と全機能のbackend交換を確認できる |

各フェーズで整形、静的解析、全テスト、全ビルド、対応する異常系・性能検証、
1440×900基準の対象アプリウィンドウ撮影と目視確認、diffレビューを行う。

## 調査済みの外部境界と選択

- Codex: 共通の `Task` / `Execution` / `TaskEvent` 契約を固定し、Codex adapter では
  app-server の JSON-RPC stdio を採用する。安定版の `codex exec --json` は非対話であり、
  入力待ち・承認待ち・実行中断を満たせないため採用しない。app-server は実験的なので、
  生成スキーマで互換性を検証し、外部の method / payload は adapter の外へ公開しない。
- Task継続: UI と同じ実行ファイルを `--task-worker` で分離起動し、Execution ごとの原子的な
  JSON snapshot と一時 control file で連携する。UI終了後も worker は継続し、再起動したUIは
  snapshotを読み直す。これはPhase 2のローカルbackend実装であり、汎用IPC・永続化方式の
  決定ではない。Phase 5で同じ `TaskBackend` 契約をloopback/remote実装へ交換して再評価する。
- Task mode: モードはrunner内の固定分岐ではなくExecutionの能力選択として保持する。
  現在はDefaultとPlanを提供し、Planだけが対話的なユーザー入力要求を利用する。
- Git: インストール済み Git CLI を adapter 内で採用した。status は
  `--porcelain=v1 -z --branch`、diff は `--no-ext-diff --no-color` の機械可読出力だけを
  型へ変換し、UIへ CLI 固有の文字列を漏らさない。Task 用 worktree は
  `%LOCALAPPDATA%\\Lapis\\worktrees` に作成し、Task ID・基点 commit・状態を
  `%LOCALAPPDATA%\\Lapis\\git-v1` に原子的に記録する。選択的な取り込みは共有 workspace
  側に同じパスの未取り込み変更がないことを確認してから行い、破棄は明示操作だけで行う。
  `git2` は別実装の候補として残し、CLI 形式の UI 露出を禁止する境界は維持する。
- LSP: 3.17 の JSON-RPC / stdio と Revision 対応を採用し、最初の server adapter を
  rust-analyzer とする。initialize、全文同期、診断、補完、定義移動、shutdown を Lapis 型へ
  変換し、古い Revision の応答を採用しない。Windows extended path を正規化して file URI を
  percent encode し、push / pull diagnostics と server request の双方を処理する。診断取得、補完、
  定義移動は UI スレッド外で実行する。検証環境には公式 stable toolchain の rust-analyzer
  component を導入し、仮想文書の実診断・補完・定義移動と shutdown を自動テストした。
- PTY: Windows ConPTY を含む cross-platform 境界として `portable-pty 0.9` を採用した。
  OS 固有型と制御シーケンスは platform adapter 内に閉じ、UI は Terminal の入力、正規化済み
  出力、resize、終了イベントだけを扱う。代替の直接 ConPTY 実装は保守範囲が大きく、単純な
  pipe は対話シェル要件を満たさない。依存追加によりビルド量とバイナリサイズは増えるため、
  配布最適化時に再計測する。
- 全文検索: WorkspaceSearchBackend を境界とし、ローカル実装は別スレッドで UTF-8 ファイルを
  走査する。再検索時は cancellation flag で旧処理を止め、`.git`、`target`、巨大・binary
  ファイルを除外する。永続索引方式は性能計測後まで固定しない。
- Conversation 永続化: `ConversationRepository` を app-services 側の契約、原子的な versioned
  JSON 実装を persistence 側に置く。Workspace、開いた文書と未保存 draft、選択・スクロール、
  パネル寸法、選択中 Execution、Terminal の cwd・寸法・終了状態を保存する。秘密情報になり得る
  Terminal 出力は保存せず、異常終了後の shell process は自動再開しない。Git 差分と診断は
  復元した Workspace / Document Revision から各 backend が再取得する。SQLite 等への移行は
  Repository 契約を維持したまま、履歴量と計測結果を見て判断する。
- リモート: frontend / backend / shared contract の分離を採用した。全 backend 契約を同じ
  `ConnectionGate` で包む loopback 実装に交換でき、切断中は型付きエラー、再接続後は保持済み
  Workspace データを返すことを検証する。transport、認証、再接続プロトコルは実 remote 実装
  まで固定しない。

## 構成

```text
crates/
├── editor-core/
├── text/
├── document/
├── workspace/
├── project/
├── task-runner/
├── terminal/
├── language/
├── lsp/
├── git/
├── persistence/
├── platform/
├── app-services/
├── desktop-ui/
└── app/
```

## 各crateの責務

### editor-core

複数の機能で使う最小限の型を置く。

- ID
- 共通Error
- 共通Event
- 基本的なPath型

機能固有の処理は置かない。

### text

テキスト編集の基礎を持つ。

- Text Buffer
- Position
- Range
- Edit
- Undo / Redo

UI、ファイル保存、LSPには依存しない。

### document

開いている文書を管理する。

- 文書の状態
- Dirty状態
- 保存状態
- Encoding
- 編集履歴

テキスト処理は`text`を使う。

### workspace

現在の作業空間を管理する。

- 開いているProject
- 開いているDocument
- Session
- Workspace設定
- 復元状態

他の機能を直接実装しない。

### project

Project単位の情報を管理する。

- Root directory
- Project設定
- File構成
- Run configuration
- Build configuration

### task-runner

長時間実行する処理を管理する。

- Codex CLI
- Claude CLI
- Build
- Test
- 任意Command
- 状態
- Log
- Cancel

Taskの状態遷移をこのcrateに置く。

### terminal

Terminal Sessionを管理する。

- PTY
- Shell process
- Input / Output
- Resize
- Session lifecycle

Terminalの表示は`desktop-ui`に置く。

### language

言語ごとの定義を管理する。

- Language definition
- File extension
- Syntax
- Language registry

### lsp

Language Serverを管理する。

- Server lifecycle
- Request / Response
- Diagnostics
- Completion
- Document synchronization

UIには依存しない。

### git

Git機能を管理する。

- Repository
- Status
- Diff
- Branch
- Commit
- Worktree

`git2`やCLIの型を外部へ漏らさない。

### persistence

永続化の実装を置く。

- SQLite
- 設定保存
- Session保存
- Workspace履歴
- Task履歴
- AI会話履歴

Repositoryの契約は、それを使う機能側に置く。

### platform

OS固有の処理を置く。

- File system
- Process
- Clipboard
- Notification
- File watcher
- Native dialog
- Window連携

大きな`PlatformService`は作らず、小さい機能へ分ける。

### app-services

複数の機能をまたぐ処理を置く。

- Workspaceを開く
- Projectを切り替える
- Document復元後にLSPを起動する
- AI Task完了後に変更Fileを再読込する

単一機能で完結する処理は、その機能側に置く。

### desktop-ui

画面と入力処理を置く。

- Window
- Editor View
- Panel
- Terminal View
- Command Palette
- View Model
- Keybinding

Domain処理、DB操作、OS操作は直接書かない。

### app

アプリ全体を組み立てる。

- Entry point
- Dependency Injection
- Event接続
- 起動処理
- 終了処理
- Lifecycle管理

`app`以外のcrateは`app`へ依存しない。

## 依存方向

```text
desktop-ui
    ↓
app-services
    ↓
各Feature

persistence ──→ 各Featureの契約
platform    ──→ 各Featureの契約

app ──→ すべてを組み立てる
```

## 禁止する依存

```text
Feature → desktop-ui
Feature → app
document → persistenceの具体実装
task-runner → OS固有APIの直接利用
crate間の循環依存
```

## Feature間の連携

同期的に完結する処理は、通常の関数やtraitを使う。

複数機能へ通知する場合はEventを使う。

```text
DocumentChanged
    ↓
app-services / Event Dispatcher
    ↓
LSP・Git・UI
```

すべてをEvent化しない。
処理の流れが明確な場合は直接呼び出す。

## crate内部の分割

小さいcrateはファイル単位で分ける。

```text
task-runner/src/
├── task.rs
├── runner.rs
├── event.rs
├── port.rs
└── lib.rs
```

大きくなった場合だけ層を分ける。

```text
task-runner/src/
├── domain/
├── application/
├── infrastructure/
└── lib.rs
```

## 公開API

各crateは`lib.rs`から必要な型だけ公開する。

他crateから内部moduleを直接参照しない。

```rust
pub use task::Task;
pub use runner::TaskRunner;
pub use event::TaskEvent;
```

## 判断基準

新しいコードは次の順で配置を決める。

1. どの機能に属するか。
2. 外部I/Oか。
3. 複数機能をまたぐか。
4. UI固有か。
5. 本当に共通化が必要か。

迷った場合は、最も近い機能のcrateに置く。

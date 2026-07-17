# ARCHITECTURE.md

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

# ARCHITECTURE.md

Lapis の論理構成と設計上の境界を定義します。Rust の具体的な実装構成は未確定です。

## 設計状態

### 確定

- デスクトップクライアントとワークスペースバックエンドを分離する。
- ローカルとリモートで同じ論理プロトコルを使う。
- 会話を中心に、編集状態、ワークスペース、AIタスクを束ねる。
- 機能の段階差はモード専用実装ではなく Capability で表す。
- エージェントは外部 CLI を実行し、状態はバックエンドが所有する。
- 主な実装言語は Rust とする。

### 未確定

- UI フレームワーク、GPU描画、テキスト編集基盤
- ローカル IPC、リモート通信、データ形式
- 非同期ランタイムと内部イベント基盤
- crate、モジュール、ディレクトリ構成
- 永続化、検索索引、共同編集、プラグインの実装方式

## 論理構成

```text
Desktop Client
  UI / Editor / Local Buffer / View State
                |
        Workspace Protocol
                |
Workspace Backend
  Files / Git / Search / LSP / Index / Terminal
  Tasks / Agent Executions / Sessions / Persistence
                |
  Local / Remote / Container Workspace
```

UI はバックエンドの配置を意識しません。ローカル利用ではクライアントがバックエンドを必要時に起動し、リモート利用では接続先だけを差し替えます。

## 責務

### Desktop Client

- 描画、入力、カーソル、選択、スクロール
- テキストバッファのローカル複製と Undo／Redo
- 開いている文書、パネル、レイアウトなどの表示状態
- 軽量な構文表示と Markdown 表示
- 会話、タスク、差分、診断の表示

通信断やバックエンド遅延があっても、入力と閲覧を可能な限り継続します。

### Workspace Backend

- ファイル、監視、検索、Git
- LSP、索引、ビルド、タスク、ターミナル
- Project、Workspace、Conversation の状態
- AI CLI の起動、入出力、承認、終了状態、ログ
- ローカル、リモート、コンテナ接続
- 永続化、再接続、リソース休止

## 中心モデル

| 概念 | 責務 |
| --- | --- |
| `Project` | リポジトリやノート保管庫などの論理単位 |
| `Workspace` | 実際に操作するローカル、worktree、リモート、コンテナ環境 |
| `Conversation` | 会話と、その会話で復元する編集・表示状態 |
| `Task` | 会話内で依頼する作業単位 |
| `Execution` | CLIやスクリプトの一回の実行 |
| `Checkout` | タスクを分離するworktreeやブランチ |
| `Document` | コードとノートに共通する編集対象 |
| `Revision` | Document の版と編集順序 |
| `Capability` | 必要時に有効化する機能 |

`Conversation` と `Task`、`Task` と `Execution` は一対一に限定しません。

## Capability

UI 上のモードは Capability のプリセットとして扱います。

```text
Note   = text + markdown + links
Editor = Note + syntax + git + basic-language-support
Smart  = Editor + full-lsp + index + inspections
Remote = 任意の構成 + remote-workspace
Pair   = 任意の構成 + collaboration
```

名称と組み合わせは暫定です。各機能は必要時に起動し、未使用時は休止または解放できる構造にします。

## Document と同期

- コードと Markdown を共通の Document として扱う。
- Document は URI、内容、言語、Revision、メタデータを持つ。
- 編集要求は基準 Revision と編集元を持つ。
- 保存済み内容、未保存内容、バックエンド上の内容を区別する。
- URI の具体的な形式は未確定とする。

## 会話とワークスペース

会話の切り替え時に、次の状態をまとめて復元します。

- Workspace とファイルツリー
- 開いている文書、カーソル、レイアウト
- Git差分、診断、ターミナル
- タスク、エージェント実行、承認待ち

同じ Workspace の読み取り処理は並列化できます。複数の書き込み処理は競合を検出し、必要に応じて Checkout を分離します。

## エージェント実行

```text
Created -> Preparing -> Queued -> Running -> Completed
                                  |  |  |
                                  |  |  +-- WaitingForInput
                                  |  +----- WaitingForApproval
                                  +-------- Paused
```

- CLIごとの差異は Adapter が共通イベントへ変換する。
- UI は標準出力を直接解析しない。
- バックエンド再接続後も、状態とログを復元できるようにする。
- 実行場所は紐づく Workspace に従う。

## 依存境界

- UI から OS、Git、LSP、AI CLI を直接呼ばない。
- ドメインモデルは UI、通信、永続化の具体技術へ依存しない。
- 外部ツールの型とイベントを中心モデルへ直接持ち込まない。
- 認証情報と秘密情報を会話、ログ、リポジトリへ保存しない。

## 設計決定の記録

未確定項目を決める際は、本文へ次を追記します。

- 決定内容
- 解決する要件
- 採用理由
- 捨てた選択肢
- 影響範囲


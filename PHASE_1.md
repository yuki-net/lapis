# PHASE_1.md

## 目的

GPUI DesktopとKMP MobileのAndroid・iOSが同じRust backendを利用し、Web技術のプロジェクトでファイル編集とTerminal実行を行える状態にします。対象画面のUI、remote動作の基盤、主要操作の安定性をPhase 1の中心とします。

完了時に、既知の重大・進行不能バグがなく、主要操作を反復でき、失敗から安全に回復できることを求めます。Desktopを基準実装とし、Mobileは基本操作に範囲を絞ります。

重大バグは、起動不能、データ損失、認証回避、Workspace外アクセス、主要フローの進行不能を指します。

## クライアントとBackend

- Desktopはlocal backendと同一プロセスで動作する。
- Mobileはremote transportでbackendへ接続する。
- 両Clientはcommand、query、event、Revisionの意味を共有する。
- Workspace、Document、Terminalの正規状態はbackendが所有する。
- 通信、cache、画面状態は各Clientがネイティブ実装する。

## 対象ファイル

- HTML
- CSS
- JavaScript、JSX
- TypeScript、TSX
- JSON
- Markdown

Phase 1ではLanguageId判定、ファイルアイコン、Encodingを維持したテキスト編集までを扱います。

## 対象機能

### UI

- 対象画面は実装前に構成、状態、操作を定め、未設計の仮UIを完成扱いにしない。
- DesktopはWorkspace、Editor、Terminal、Settings、接続状態を表示できる。
- Mobileは接続、Files、Editor、Terminal、Settingsを画面として提供する。
- 各画面に通常、loading、empty、error、disabled、disconnectedの必要な状態を用意する。
- 利用不能な機能を有効なボタンやメニューとして表示しない。

### 基本システム

- DarkとLightのThemeを切り替えられる。
- UI言語を切り替えられる。
- Theme、UI言語、Language定義を登録で追加できる。
- UI表現の共通規則を`DESIGN.md`に定義する。

### WorkspaceとDocument

- Workspaceを開く、閉じる、切り替える。
- FilesからDocumentを開く。
- 新規作成、編集、保存、undo、redoを行う。
- dirty状態とRevisionを維持する。
- 古い応答と競合する保存を安全に拒否する。
- Clientの再起動後にWorkspaceと未保存Documentを復元する。

### Terminal

- backendがTerminalの開始、入力、出力、resize、終了を所有する。
- DesktopからWeb開発用commandを実行できる。
- MobileからTerminalを開始し、基本的な入出力を行える。
- 切断時に実行状態を誤って再開済みとして扱わない。

### Mobile Platform

- 共有ロジックは`commonMain`を基本とする。
- Androidを日常的な実装・build・UI確認の基準にする。
- Android実機がない場合はemulatorで主要操作を確認する。
- iOS実機操作はiPhoneで最終確認する。
- MacとXcodeを利用できる節目ではiOS build互換を確認し、問題をPhase末まで蓄積しない。
- platform固有処理は`androidMain`と`iosMain`へ閉じる。

### Remote基盤

- 利用者が明示的にbackendのremote接続を有効化する。
- 未認証の接続を許可しない。
- Phase 1の正式な接続範囲は同一LAN内とし、インターネットへの直接公開を要求しない。
- Protocol versionとClient capabilityを接続時に確認する。
- 認証情報、Document内容、Terminal入出力を平文で送らない。
- 接続、切断、再接続を画面状態として表示する。
- transport固有の型とエラーをcoreやUIへ漏らさない。
- Workspace外のpathと、Terminal capabilityがないClientからの開始要求を受け付けない。
- MobileからFiles閲覧、基本編集、保存、Terminal操作を行える。
- Desktopと同等のPanel、キーバインド、大規模編集性能は要求しない。

## 実装順序

1. `DESIGN.md`、`REMOTE.md`と基本systemの契約を定める。
2. 対象画面と各状態を確定する。
3. Workspace、Document、TerminalのClient契約を整理する。
4. Desktopで基本Web開発の流れを完成させる。
5. remote transportと認証を追加する。
6. KMP Androidで基本操作を接続する。
7. iOS build互換を確認し、iPhoneで最終操作を確認する。
8. 競合、切断、復元と回帰動作を検証する。

各段階は単独で動作する変更として統合し、Phase全体を一つの長期ブランチで実装しません。

## 詳細設計

- `DESIGN.md`は画面、状態、共通tokenとClientごとの表現を所有する。
- `REMOTE.md`は信頼境界、認証、暗号化、接続状態、version、capability、再接続を所有する。
- 具体的なmessageと型はSchemaまたはコードを正とし、文書へ重複させない。
- 個別機能の一時計画はissueや依頼文で管理し、恒久文書を増やさない。

## 対象外

- Web client
- インターネット公開、Cloud relay、Account同期
- シンタックスハイライト
- LSP、Completion、Diagnostics
- Git UI、worktree操作
- AI Agent
- Plugin、共同編集
- DesktopとMobileの完全なUI共通化

## 受入条件

- 対象画面の通常状態と必要なloading、empty、error、disconnected状態が実装されている。
- Desktopで対象ファイルを開き、編集、保存、再起動復元できる。
- DesktopのTerminalからWeb開発用commandを開始、操作、停止できる。
- Androidが同一LAN内で認証付きのbackendへ接続し、Files閲覧、基本編集、保存、Terminal入出力を行える。
- iPhone実機で接続、Files閲覧、基本編集、保存、Terminal入出力を最終確認できる。
- Theme、UI言語、LanguageIdがハードコード分岐を増やさず追加可能である。
- 複数Clientの古いRevisionを黙って保存せず、競合として扱う。
- Workspace外のpathと、Terminal capabilityがないClientからの開始要求を拒否できる。
- 主要操作の自動テストと回帰テストがあり、既知の重大・進行不能バグがない。
- Rustの必須検証が成功し、KMPは検証基盤の状態を明記する。

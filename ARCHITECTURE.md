# ARCHITECTURE.md

Lapis の長期的な設計境界を定義します。現在のファイル一覧や実装進捗ではなく、コードだけでは判断できない理由と不変条件を記録します。

## システム境界

Lapis は、Rust backend を GPUI Desktop、KMP Mobile、Vite Web から利用する構成です。Desktop は軽さと即応性を優先し、現時点では local backend と同一プロセスで動かします。ただし UI と backend の契約は、将来 daemon や remote transport へ置き換えられる境界を保ちます。

トップレベルは実行主体と責務で分けます。

- `apps/`: Desktop、Mobile、Web 各クライアント。画面とクライアント固有の状態を所有する。
- `backend/core/`: プラットフォームに依存しないモデル、不変条件、アプリケーション契約を所有する。
- `backend/local/`: ファイルシステム、プロセスなどローカル環境との接続を実装する。
- `backend/persistence/`: 永続化契約の具体実装を所有する。
- `features/`: Git、LSP、Terminal など、core から独立して有効化できる機能を所有する。
- `vendor/`: 外部コード。Lapis 固有の責務を追加しない。

未実装機能の空ディレクトリは作りません。ディレクトリの存在は、少なくとも契約または動作する実装があることを意味します。

## 依存方向

依存は UI と具体的な外部環境から、安定した Lapis の契約へ向けます。

```text
apps ────────────────┐
backend/local ───────┼──> core / feature contracts
backend/persistence ─┘

app entry point ──> concrete adapters を組み立てる
```

- `core` は GPUI、Android、Browser、OS、通信方式、永続化方式へ依存しない。
- feature 間で循環依存を作らない。連携が必要なら、所有者の契約か application 層で調停する。
- 外部ライブラリ、CLI、SDK の型とエラーは adapter で Lapis の型へ変換する。
- 具体実装を選択する composition は各 app の entry point に置く。
- UI が契約の型を参照することはよいが、adapter の具体型や外部 I/O を参照してはならない。

## Core と Feature

`core` は、特定機能を無効にしても成立する概念と不変条件を持ちます。`feature` は、独立した起動条件、状態、外部資源、停止処理を持つ利用者向け能力です。

機能の `installed`、`loaded`、`running` は別の状態として扱います。画面を表示していないだけで、未実装機能が利用可能であるかのように扱ってはいけません。機能間連携は capability と明示的な契約で行い、core を機能固有の分岐で増やしません。

## 状態の所有

backend は Workspace、Document、Git、LSP、Terminal、Task、Execution など、複数クライアントで一致すべき正規状態を所有します。client は focus、selection、scroll、panel、navigation、描画用 cache など、その画面にだけ必要な状態を所有します。

同じ状態を複数の所有者が独立して更新してはいけません。client に複製する正規状態は snapshot または event から得た cache とし、再接続時に置き換えられるものにします。

文字入力など即時性が必要な操作は client で先に反映できます。その場合も Document の Revision を基準に backend と整合し、古い応答や診断を新しい状態へ適用しません。

## 中心概念

- `Project`: 設定と履歴を束ねる論理単位。
- `Workspace`: ファイルや開発機能を提供する実行環境。同じ Project に複数存在できる。
- `Conversation`: 復元可能な対話と画面文脈。
- `Task`: 利用者が依頼した論理的な仕事。
- `Execution`: Task を特定の環境と権限で実行した一回の試行。

これらは ID とライフサイクルを別に持ちます。特に Conversation を画面タブ、Task をプロセス、Project をディレクトリの別名として実装しません。

## Document と Revision

Document は内容、保存済み位置、現在 Revision、保存済み Revision、外部ファイルの識別情報を持ちます。内容を変更する transaction ごとに Revision を進めます。

- 保存、LSP、検索、remote 同期の結果には対象 Revision を関連付ける。
- 保存は既知の Revision と外部状態を比較し、競合を黙って上書きしない。
- undo / redo はファイル分割ではなく、利用者にとって一つの操作である transaction を単位にする。
- selection、caret、scroll は Document 内容ではなく client の view state とする。

## UI と Backend の契約

境界は command、query、event、snapshot で表現します。local と remote は transport が異なっても意味を共有します。

- command は状態変更の意図、query は読み取り、event は確定した変化を表す。
- メッセージには必要に応じて ID、Revision、順序情報を含める。
- Git CLI、LSP、OS、通信ライブラリの出力をそのまま公開しない。PTY は端末 emulator が解釈する専用の raw stream 契約を通す。
- UI は結果の表示方法を決めるが、業務上の正規状態や回復方針を決めない。

Desktop、Mobile、Web で共有するのは、契約、状態の意味、生成可能な型です。通信、cache、画面状態は各 client のネイティブ実装を基本とします。FFI や WASM による client logic の共有は、重複コストが境界維持コストを実測で上回った場合に再検討します。

Terminal の PTY output は表示用 text ではなく、順序番号付きの byte chunk として契約を越える。ANSI/VT sequence、alternate screen、read 境界を跨ぐ UTF-8、非 UTF-8 byte は client の terminal emulator が解釈するため、backend と transport は変換・除去しない。snapshot は再接続時に再利用できる範囲として raw output の末尾、最後の output sequence、保持上限による欠落状態、size と lifecycle を持つが、終了した PTY process 自体は再開しない。これにより Local と Remote は transport が異なっても同じ ordered stream semantics を共有できる。

## 設計判断の記録

非同期 runtime、IPC、remote transport、認証、永続化の詳細、共同編集、拡張 package は未確定です。実装に必要になるまで固定しません。

長期的な制約を追加するときは、採用理由、退けた選択肢、影響範囲、再検討条件をこの文書へ簡潔に記録します。一時的な実装状況、依存バージョン、実行コマンド、詳細なファイル構成はソースや履歴に委ねます。

見た目、視覚的一貫性、UI コンポーネントの表現規則は、作成後の `DESIGN.md` が所有します。製品要件や作業優先度は、この文書では管理しません。

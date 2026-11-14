# ACP 拡張

Lapis から外部ハーネス（コーディングエージェント）へ接続するための ACP（Agent Client Protocol）client 境界です。Task の正規状態や Workspace の判断は持ちません。

## 構成

- `host/extensions/acp/`（crate `local-acp`）: ACP の client 実装。ハーネス起動、`initialize` / `session/new` / `session/prompt` の送信、`session/update` / `session/request_permission` / `session/cancel` の受信と、`task::TaskEvent` への変換を所有します。
- `extensions/acp/contract/`（crate `acp-contract`）: core と Client が知る必要がある runner 識別子（`ACP_RUNNER`）だけを公開します。ACP の型は置きません。

ACP 固有の型・起動引数・更新の解釈は `local-acp` の内側に閉じ込め、外へは `task` の値（`ExecutionStatus` / `TaskEvent`）だけを渡します。

## 対象範囲

ACP の安定面の最小セットだけを扱います。

- `initialize`、`session/new`、`session/prompt`
- `session/update` 通知の受信
- `session/request_permission` への応答（種類で選び、常に狭い方を優先）
- `session/cancel` による取消

対象外は ACP Registry 対応、複数プロバイダ同時接続、Subagent Sessions RFD です。

## 最初のハーネス

OpenCode のみを対象にします。起動方法は `opencode acp`（stdio の改行区切り JSON-RPC）で、実機で確認済みです。`session/new` の `cwd` が agent のツール実行基準になります。実行ファイルは `LAPIS_OPENCODE_PATH`、無ければ npm の実体 `opencode.exe` を解決します。

## データの流れ

1. Task の `runner` が `acp-opencode` のとき、Task worker は `run_acp_session` を選ぶ。
2. `local-acp` が ACP agent を起動し、`session/update` を `TaskEvent` に変換して返す。
3. worker が `TaskRecord` へ反映・永続化する。
4. Host の Task 変更通知で Client が再取得し、既存の Task 画面が表示する。

## 検証

- `cargo test -p local-acp`: 変換ロジックと、`fake-acp-agent` を使った client の end-to-end 検証。
- `cargo test -p local-tasks --test acp_acceptance`: 実 OpenCode がある環境で、本番と同じ worker 経路を通す acceptance 検証（無い環境では skip）。

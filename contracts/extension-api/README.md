# 拡張公開契約

拡張開発者と拡張ホストが共通に使う WIT の正本。`wit/v1` はライフサイクル、
`wit/pty-v1` は汎用 PTY セッションの契約である。PTY を使う package は `requires` に
`lapis.process.pty` 1.0.0 を、`capabilities` に `process.pty` 1.0.0 を宣言する。
能力は承認が必要で、現在は Windows Host だけが提供する。拡張固有の Rust 型や Host の
内部参照は境界を越えない。互換性を壊す変更は既存の v1 を書き換えず、別の版を追加する。
PTY の作業ディレクトリは認証済み Workspace からの相対パスとし、Host が実パスへ解決する。
Workspace 外へ出るパスは拒否し、接続情報のないライフサイクル呼び出しでは起動できない。

`wit/process-info-v1` は Host の PTY 種別と既定シェルを返す汎用プロセス契約。
利用には `lapis.process.info` 1.0.0 と `process.pty` 能力の宣言・承認が必要。
拡張が管理するユーザーの shell profile はこの契約に含めない。

`wit/dispatch-v1` は拡張への汎用操作の契約で、操作名とデータの意味は個別の契約が定める。
Session と Workspace は Host が認証済み接続から付与し、要求 payload の値は信用しない。
`lapis.system.` は Host 専用の操作名であり、クライアントからは呼び出せない。
接続の終了時は `lapis.system.session-detached` を通知する。拡張は同じ Session が
保有する入力権限などを解放し、プロセスや永続状態を終了しない。通知の過負荷時は
`lapis.system.release-claims` に集約され、全 Session の一時的な所有権を解放する。
どちらも payload は空の JSON オブジェクトで、未対応の拡張は無視してよい。
実行中の署名済み manifest が `lapis.extension.dispatch` 1.0.0 を要求する場合に使える。
Workspace 間の拡張状態はまだ分離されていないため、Host 全体の拡張を操作する
`extensions.execute` 権限、または Host 管理権限が必要。Workspace 限定の読み取り権限では呼び出せない。

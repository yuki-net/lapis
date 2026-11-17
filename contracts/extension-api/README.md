# 拡張公開契約

拡張開発者と拡張ホストが共通に使う WIT の正本。`wit/v1` はライフサイクル、
`wit/pty-v1` は汎用 PTY セッションの契約である。PTY を使う package は `requires` に
`lapis.process.pty` 1.0.0 を、`capabilities` に `process.pty` 1.0.0 を宣言する。
能力は承認が必要で、現在は Windows Host だけが提供する。拡張固有の Rust 型や Host の
内部参照は境界を越えない。互換性を壊す変更は既存の v1 を書き換えず、別の版を追加する。

`wit/dispatch-v1` は拡張への汎用操作の契約で、操作名とデータの意味は個別の契約が定める。
実行中の署名済み manifest が `lapis.extension.dispatch` 1.0.0 を要求する場合に使える。
Workspace 間の拡張状態はまだ分離されていないため、Host 全体の拡張を操作する
`extensions.execute` 権限、または Host 管理権限が必要。Workspace 限定の読み取り権限では呼び出せない。

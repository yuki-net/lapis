# 拡張公開契約

拡張開発者と拡張ホストが共通に使う WIT の正本。`wit/v1` はライフサイクル、
`wit/pty-v1` は汎用 PTY セッションの契約である。PTY を使う package は `requires` に
`lapis.process.pty` 1.0.0 を、`capabilities` に `process.pty` 1.0.0 を宣言する。
能力は承認が必要で、現在は Windows Host だけが提供する。拡張固有の Rust 型や Host の
内部参照は境界を越えない。互換性を壊す変更は既存の v1 を書き換えず、別の版を追加する。

`wit/dispatch-v1` は拡張への汎用操作の契約で、操作名とデータの意味は個別の契約が定める。
現在は拡張ホスト内の経路を検証中であり、Host はこの契約をまだ外部に提供していない。

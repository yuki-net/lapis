# 拡張公開契約

`native/` は同じ target の拡張ホストとネイティブ拡張が共有する ABI v1 の試作。
`lapis_extension_v1` の関数表と `abi_version` / `struct_size` を照合し、操作と
Host 能力の引数・応答は UTF-8 の JSON とする。入力領域は呼び出し中だけ有効で、
応答領域は確保した側の `release` で解放する。Host の context は不透明な値であり、
拡張は参照しない。互換性を壊す版は別の entry symbol と型を追加する。
この ABI の package 読み込み・署名照合・権限管理への接続は未実装。
ネイティブコードは ABI だけでは OS アクセスを制限できず、既存の Wasm 実行とは
隔離の性質が異なる。

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

`lapis.extension.views` 1.0.0 は `views.json` による表示宣言であり、設定・翻訳の
`lapis.extension.contributions` とは独立して版を持つ。型の正本は
`contracts/domain/extension-ui` の `StaticViews`。ID と command は提供元の名前空間に
属し、title は同じ package の既定言語に存在しなければならない。Host は検証した宣言を
Client に再配布する。描画器への接続とアイコン配布はまだ実装されていない。

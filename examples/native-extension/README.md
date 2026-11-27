# native extension example

汎用の native 拡張 runtime を検証するための最小例です。公開 native ABI v1 の
`echo.v1.echo` だけを実装し、Terminal の PTY やセッション操作には依存しません。
配布はせず、テスト時にローカルでビルドして fixture として読み込みます。

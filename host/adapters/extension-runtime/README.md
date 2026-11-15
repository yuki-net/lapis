# 拡張ホスト実行

Wasm component のライフサイクルを隔離した拡張ホストプロセスで実行するための基盤。
`contracts/extension-api/wit/v1` の `activate` と `deactivate` を使う。
Host との間は JSON 行で bytes と結果だけをやり取りし、Host の参照を渡さない。
この段階では Host の起動・監督や `.lapis` の検証、権限を伴う能力 API は接続していない。

# 拡張ホスト実行

Wasm component のライフサイクルを隔離した拡張ホストプロセスで実行するための基盤。
`contracts/extension-api/wit/v1` の `activate` と `deactivate` を使う。
Host は署名と互換性を検証した component を別プロセスに渡し、終了時は一度だけ再起動する。
JSON 行の双方向 call/reply は呼出 ID と現在実行中の拡張 ID を照合し、Host の参照を渡さない。
Windows Host は承認された拡張に PTY v1 を提供する。未承認の呼び出しは拒否し、
承認の取り消し・拡張ホストの停止時にはその拡張の PTY を回収する。Windows では
通常のプロセス生成による子孫を Job で管理する。ほかの OS は PTY 能力をまだ提供しない。
承認の設定画面と、作業中のリソースを終了確認へつなぐ処理は未完成である。

native ABI の読み込みは試作段階で、検証済み bytes を別プロセスに渡せるが、通常の package 起動経路には未接続。
Windows では検証済み bytes を一時 file に固定・再照合し、依存 DLL は system32 から探す。
これは native コードの OS 操作を制限する sandbox ではなく、実行コードの信頼判断は呼び出し側が担う。

`python -m scripts.extensions.smoke_pty` は独立 workspace の Wasm component をビルドし、
模擬 Host を使って IPC と拒否の両方を検証する。`wasm32-wasip2` target が必要。

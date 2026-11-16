# 拡張ホスト実行

Wasm component のライフサイクルを隔離した拡張ホストプロセスで実行するための基盤。
`contracts/extension-api/wit/v1` の `activate` と `deactivate` を使う。
Host は署名と互換性を検証した component を別プロセスに渡し、終了時は一度だけ再起動する。
JSON 行の双方向 call/reply は呼出 ID と現在実行中の拡張 ID を照合し、Host の参照を渡さない。
PTY WIT と模擬 Host の通信は検証できるが、実 OS の PTY 能力はまだ提供していない。
子孫プロセスまで確実に監督できるようになるまで、Host の handler は既定で呼び出しを拒否する。

`python -m scripts.extensions.smoke_pty` は独立 workspace の Wasm component をビルドし、
模擬 Host を使って IPC と拒否の両方を検証する。`wasm32-wasip2` target が必要。

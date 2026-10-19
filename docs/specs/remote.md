# Remote — internal constraints and design notes

この文書はRemoteの**安全上の制約**と**未確定の設計案**を区別します。「実装済み」は主に `backend/transport/remote/` のServer側でコードから確認できる範囲を指し、Clientとの連携まで完成している意味ではありません。

## 信頼境界

- LAN内の接続元であっても信用しません。Remoteは利用者が明示的に有効化する設計とします。
- 認証前にWorkspace等の情報を返さず、認証後も操作ごとのcapabilityを実行側が確認します。
- Client由来のpathをそのままOSへ渡さず、Workspaceと検証済みの相対pathから解決し、親参照・symlink等による範囲外アクセスを拒否します。
- Document・Terminalなどのresource identityを認証session・Workspaceへ関連付け、別sessionから利用させません。
- 認証情報、ファイル内容、Terminalの入出力を不用意にログへ記録しません。

## Server側で実装済みの制御

- `server.rs` と `tls.rs`: Remoteが有効化されていなければlistenしません。WebSocketのServerはTLS 1.3のみを許可し、TLS early dataを無効にしています。
- `auth.rs`: Pairing Tokenには有効期限があり、照合時に取り除くことで再使用を防ぎます。発行するcredentialはClient・Workspace・許可capability・期限に結び付き、失効させると関連sessionも無効になります。
- `auth.rs`: 認証済みsessionの有効性と、requestが要求するWorkspace・capabilityを検証します。

これらはServer側の実装の説明であり、Clientの証明書検証、認証情報の安全な永続化、完成した接続体験を保証するものではありません。現在のcredential管理は `RemoteAuth` 内のメモリ上の状態です。

## Client連携などの未確認・未確定事項

- ClientがServer証明書のfingerprintをPairing時に受け取り、その後もpinningする仕組み。
- QR・手動codeによるPairing招待の配布と、Client資格情報の安全な永続化。
- Server秘密鍵・証明書の保存と更新手順。

これらは現行のServer実装だけでは完了を確認できません。具体化するときに実装・検証し、認証拒否、protocol不一致、証明書不一致を単なる一時切断として自動的に信頼し直さないこと。

## 状態と回復

- 接続が失われても、実行Host上のTerminalやAgentが終了したとは推測しません。
- 再接続では権限と実行状態を再確認し、古いClient cacheを最新のsnapshotに整合させます。
- Documentの変更・保存はrevision等を使って競合を検出し、古い内容で黙って上書きしません。
- Terminal出力は順序とbyte表現を保ち、通信層で勝手に整形・除去しません。
- Messageサイズ、request数、session数、timeoutなど、必要な資源上限を設けます。

## 最低限の確認項目

未認証・権限不足・Workspace外path・失効済credentialを拒否できるか、stale revisionを検出できるか、切断中の実行状態を誤認しないか、再接続時にcacheを更新できるかを検証します。実行可能な検証手段の正本はテストコードとCIです。

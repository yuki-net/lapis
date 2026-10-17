# Remote — internal constraints and design notes

この文書はRemote実装の**安全上の設計意図**を保存します。現行アプリにすべて実装されているという意味ではありません。通信方式、認証フローの具体化は機能実装時に現状のコードと合わせて再確認します。

## 信頼境界

- LAN内の接続元であっても信用しません。Remoteは利用者が明示的に有効化する設計とします。
- 認証前にWorkspace等の情報を返さず、認証後も操作ごとのcapabilityを実行側が確認します。
- Client由来のpathをそのままOSへ渡さず、Workspaceと検証済みの相対pathから解決し、親参照・symlink等による範囲外アクセスを拒否します。
- Document・Terminalなどのresource identityを認証session・Workspaceへ関連付け、別sessionから利用させません。
- 認証情報、ファイル内容、Terminalの入出力を不用意にログへ記録しません。

## 接続と権限の設計案

初期案はTLSで保護したWebSocketと証明書pinning、短命の一回限りのPairing招待、端末ごとの失効可能なcredentialです。詳細手順や保存先、暗号方式は実装前の安全性検討・テストを必要とし、現段階で実装済みとみなしません。

Protocol versionとcapabilityを接続時に交渉し、未対応の操作を誤って実行しないこと。資格情報の失効、プロトコル不一致、証明書不一致を単なる一時切断として自動的に信頼し直さないこと。

## 状態と回復

- 接続が失われても、実行Host上のTerminalやAgentが終了したとは推測しません。
- 再接続では権限と実行状態を再確認し、古いClient cacheを最新のsnapshotに整合させます。
- Documentの変更・保存はrevision等を使って競合を検出し、古い内容で黙って上書きしません。
- Terminal出力は順序とbyte表現を保ち、通信層で勝手に整形・除去しません。
- Messageサイズ、request数、session数、timeoutなど、必要な資源上限を設けます。

## 最低限の確認項目

未認証・権限不足・Workspace外path・失効済credentialを拒否できるか、stale revisionを検出できるか、切断中の実行状態を誤認しないか、再接続時にcacheを更新できるかを検証します。実行可能な検証手段の正本はテストコードとCIです。

# Remote — internal constraints and design notes

この文書はRemoteとLocal QUIC接続の信頼境界・接続寿命を定義します。操作手順は `backend/README.md`、実行時挙動と検証はコード・テストを正本とします。

## 信頼境界

- LAN内の接続元であっても信用しません。Remoteは利用者が明示的に有効化する設計とします。
- 認証前にWorkspace等の情報を返さず、認証後も操作ごとのcapabilityを実行側が確認します。
- Client由来のpathをそのままOSへ渡さず、Workspaceと検証済みの相対pathから解決し、親参照・symlink等による範囲外アクセスを拒否します。
- Local所有者だけが持つ `host.manage` は、フォルダの登録とWorkspace間のDocument移動を許可します。登録時のみ利用者が指定した絶対pathを渡し、Hostが実在・正規化を検証します。通常のresource操作は相対pathとIDを使い、Remote招待にはこの管理権限を付与しません。所有者も各sessionの通常操作は接続Workspaceに限定します。
- Document・Terminalなどのresource identityをWorkspaceとsessionのattachmentへ関連付けます。同じWorkspaceを許可されたClientはsnapshotから再アタッチできます。別Workspaceや未認証sessionからの利用は拒否します。
- 認証情報、ファイル内容、Terminalの入出力を不用意にログへ記録しません。

## Server側で実装済みの制御

- `server.rs`・`quic.rs` と `tls.rs`: Remoteが有効化されていなければlistenしません。WebSocketとQUICはTLS 1.3のみを許可し、TLS early dataを無効にしています。認証・認可・イベント配信は同じSession処理を使用します。
- `auth.rs`: Pairing Tokenには有効期限があり、照合時に取り除くことで再使用を防ぎます。Remote credentialはClient・Workspace・許可capability・期限に結び付き、失効させると関連sessionも無効になります。Local所有者credentialのみHost内の別Workspaceへ認証できますが、Client bindingは維持します。
- `auth.rs`: 認証済みsessionの有効性と、requestが要求するWorkspace・capabilityを検証します。
- 購読イベントを送る前にもsessionの失効・期限を確認します。失効したClientへ新しいDocumentやTerminalの内容を配信しません。

## QUICのwireと接続寿命

- ALPNは `lapis.v0`。1接続につき1本のClient起点bidirectional streamを使い、各方向で4 byte big-endianのpayload長に続けてUTF-8 JSONの `ClientMessage` / `ServerMessage` を送ります。
- 長さを検証してからpayloadを読み、受信の途中でイベント配信へ切り替わってもcodecに部分frameを保持します。message上限・認証timeout・idle timeout・送信timeoutと、endpointあたり128接続の上限を設けます。
- Clientは利用者が信頼して渡した公開証明書とServer名を検証し、不一致を拒否します。証明書検証の省略や0-RTTでの認証・更新は行いません。
- 切断時は認証sessionとattachmentのみを解除します。RuntimeのDocument・PTYはHostが所有し、再認証とWorkspace接続後にsnapshotから復元します。送信失敗後のstreamは再利用しません。
- Host側の秘密鍵・credential検証情報はメモリ内に限ります。Host再起動を跨ぐ実行資源や資格情報の復元は実装していません。

Desktopは招待ファイルから証明書と一回限りのtokenを取り込み、以後は証明書とcredentialを保存したprofileを使います。Windowsではユーザー単位DPAPI、Unixの開発・CIではprivate directory/fileで保護します。Local所有者情報は親子の匿名pipeだけで受け渡し、引数やログへ秘密を出しません。Local所有者の鍵はOSユーザーとHostの寿命に紐付き、Remote credentialとは期限を分けます。

管理Local Hostは、起動した子プロセスのPIDとの一致と、Host自身が終了まで保持する`local-v1.lease`の排他ロックで起動と寿命を確認します。OSのprocess開始情報は取得できる場合のみ記録します。OSがロックを解放していれば、古いPIDの再利用先へアクセスできなくても起動し直せます。ロックが保持されている間は通信失敗だけで置換しません。ロック情報のない旧Local記録はPIDと開始情報で確認し、同一性を証明できない場合は誤置換を避けて明示的な再起動を案内します。

## Clientの回復

- `sync_client.rs` は機能workerから呼び出し、再接続では保存済みの証明書・credentialを検証し、Workspace接続とsnapshot取得でattachmentを復元します。送信済み更新要求は自動再送しません。期限を過ぎた待機要求は実行しません。
- イベント購読のqueue上限を超えた場合は購読を切断し、snapshotから回復します。

認証拒否、protocol不一致、証明書不一致を単なる一時切断として自動的に信頼し直しません。

## 未実装事項

- QR・手動codeによるPairing招待の配布。
- Server秘密鍵・証明書の保存と更新手順。

## 状態と回復

- 接続が失われても、実行Host上のTerminalやAgentが終了したとは推測しません。
- 再接続では権限と実行状態を再確認し、古いClient cacheを最新のsnapshotに整合させます。
- Documentの変更・保存はrevision等を使って競合を検出し、古い内容で黙って上書きしません。
- Terminalの画面・mode・端末問い合わせへの応答はHostのTerminal Engineが所有します。ClientはSnapshot / Deltaのrevisionを検証し、欠落時には最新Snapshotへ再同期します。raw VT/ANSIやbyte offsetをClientへ公開しません。
- Messageサイズ、request数、session数、timeoutなど、必要な資源上限を設けます。

## 最低限の確認項目

未認証・権限不足・Workspace外path・失効済credentialを拒否できるか、stale revisionを検出できるか、切断中の実行状態を誤認しないか、再接続時にcacheを更新できるかを検証します。実行可能な検証手段の正本はテストコードとCIです。
## Terminal の同期契約

Terminal の正規状態は Host が所有する。PTY の raw bytes は Host 内部の
Terminal Engine にだけ渡し、Client へは `TerminalSnapshot` と
`TerminalDelta` で構成した Lapis Terminal Model を送る。Client は raw VT/ANSI
を解釈せず、受信した screen、cursor、mode、revision を表示用 cache に適用する。

- 初回 attach、再接続、revision gap、状態不一致では Snapshot を使う。
- 通常更新は連続した `TerminalDelta.revision` の changed rows を使う。
- Client から Host へは `TerminalInput::Text`、`Key`、`Paste` を送り、PTY bytes
  への変換と bracketed paste の処理は Host の Terminal Engine 境界で行う。
- attach、表示、focus、別 Client の接続では PTY size を変更しない。明示的な
  resize 操作だけが canonical size と Engine size を更新し、全 Client に通知する。
- Client の切断、Window の close、画面の切替は Terminal process を終了させない。

この契約により、Desktop は GPUI Renderer、将来の Mobile は native Renderer として
同じ Model を描画できる。Agent Session、ACP、Chat UI、provider 固有の handoff は
Terminal の同期契約に含めない。

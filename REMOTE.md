# REMOTE.md

Lapisのremote接続における信頼境界、認証、暗号化、接続状態、互換性、回復方針を定義します。wire messageの完全な一覧と型は`lapis-client-api`およびremote adapterのコードを正とします。

## 範囲

Phase 1は、利用者が所有するDesktop backendへAndroidまたはiOS clientから同一LAN内で接続する構成を対象とします。

- backendは利用者が明示的に有効化したときだけlistenする。
- Internetへの直接公開、Cloud relay、Account同期は対象外とする。
- Desktopのlocal利用はremote transportを経由しない。
- MobileとDesktopはcommand、query、event、Revision、errorの意味を共有する。

## 信頼境界

LAN内の端末と通信は信頼しません。接続元、DNS、IP address、router、同一LAN上の他端末を認証根拠にしません。

- 認証前のclientへWorkspace情報を返さない。
- 認証済みclientにも付与されていないcapabilityを許可しない。
- clientからabsolute pathを受け取らない。
- Workspace IDと検証済みrelative pathからbackend側で実pathを解決する。
- symlinkを含む解決後pathがWorkspace外へ出る場合は拒否する。
- Terminal、Document、Subscriptionなどのresource IDは認証sessionとWorkspaceへ関連付ける。
- transport固有の型、error、再試行をcore、feature、UIへ漏らさない。

## 暗号化

Remote通信はTLS 1.3上のWebSocketを使用し、`wss`以外の接続を許可しません。

- backendはinstallationごとの秘密鍵と証明書を生成して保護する。
- Mobileはpairing時に受け取った公開鍵fingerprintを保存し、以後pinningする。
- 証明書検証を無効化するdebug経路を製品buildへ含めない。
- TLS early dataを使用しない。
- 認証情報、Document内容、Terminal入出力をlogへ記録しない。

証明書を更新する場合は、認証済み接続上で新旧fingerprintを明示して更新するか、再pairingを要求します。検証失敗時に自動で新しい証明書を信頼しません。

## Pairingと認証

PairingはDesktopが表示するQRまたは手動codeから開始します。招待には接続先、証明書fingerprint、一回限りの十分に長いtoken、有効期限、protocol majorを含めます。

1. 利用者がDesktopでremote接続とpairingを明示的に有効化する。
2. Desktopが短時間だけ有効なpairing invitationを生成する。
3. Mobileがinvitationから証明書をpinningしてTLS接続する。
4. backendがone-time tokenを検証し、tokenを即時失効させる。
5. backendがclient IDと許可capabilityに関連付けたcredentialを発行する。
6. MobileはcredentialをAndroid KeystoreまたはiOS Keychainへ保存する。

再接続ではcredentialとpinning済み証明書の両方を検証します。credentialはbackend側から個別に失効でき、失効後は再pairingを要求します。短い表示用codeだけを認証秘密として使用しません。

## Protocol互換性

Protocol versionは`major.minor`で表します。

- major不一致は接続を拒否する。
- 同じmajorでは双方が対応するminorの共通範囲から一つを選ぶ。
- 未知のcapabilityは無視し、付与されていないcapabilityのmessageは拒否する。
- messageにはrequest IDを付け、responseと対応させる。
- errorは安定したcodeと利用者向けではない詳細を分ける。
- 不明な必須field、上限を超えるmessage、順序違反はprotocol errorとして切断できる。

Handshake完了前はpairing、認証、version negotiationに必要なmessageだけを受け付けます。

## Capability

Capabilityはclientの希望ではなく、backendが認証sessionへ付与した権限を正とします。Phase 1では少なくとも次を分離します。

- Workspace一覧と接続
- Filesの読み取り
- Documentの読み取り
- Documentの編集と保存
- Terminalの開始
- Terminalの入出力、resize、終了

UIは付与されたcapabilityから操作可能性を決めます。未付与操作を非表示または理由付きdisabledにしても、backendで必ず再検証します。

## DocumentとRevision

backendがDocument内容とRevisionの正規状態を所有します。

- open時にDocument ID、content、Encoding、Revisionをsnapshotとして返す。
- editとsaveはclientが基準にしたRevisionを含める。
- backendのRevisionと一致しない変更を黙って適用しない。
- conflictは現在Revisionと再同期が必要であることを返す。
- reconnect後はbackend snapshotでclient cacheを置き換える。
- client側の未送信変更は別に保持し、snapshotへ暗黙mergeしない。

## Terminal

backendがPTY processとTerminal lifecycleを所有します。

- Terminal開始時にWorkspaceと`workspace.terminal` capabilityを検証する。
- cwdはWorkspace内の検証済みrelative pathから決める。
- input、resize、terminateはsessionに関連付いたTerminal IDだけを受け付ける。
- output eventにはTerminal IDと単調増加するsequenceを付ける。
- reconnect時にprocessが生存していると推測しない。
- backend snapshotでstatusを確認し、取得できないoutputの欠落を明示する。

## 接続状態

Clientは次の状態を区別します。

```text
Disconnected
  -> Discovering / Connecting
  -> Pairing
  -> Authenticating
  -> Negotiating
  -> Synchronizing
  -> Connected
  -> Reconnecting
  -> Synchronizing
  -> Connected
```

認証拒否、version不一致、証明書不一致、利用者による切断は自動retryしません。一時的なnetwork切断だけを上限付きbackoffで再試行し、利用者はcancelできます。

## 再同期

再接続は切断前のclient状態をそのまま有効化する操作ではありません。

1. sessionとcapabilityを再確立する。
2. Workspace lifecycleを確認する。
3. open DocumentとRevisionのsnapshotを取得する。
4. Terminalの実在とstatusを取得する。
5. subscriptionの開始位置を確認する。
6. snapshot適用後にConnectedへ遷移する。

event履歴を連続して再生できない場合は、欠落を隠さずsnapshot同期へ切り替えます。

## Resource制限

backendは認証済みclientにも上限を適用します。

- message byte数
- 同時request数
- open Document数
- Terminal session数
- request timeout
- pairing試行回数
- 認証失敗回数

上限値は実装の設定を正とし、この文書へ重複させません。

## 保存

- backend秘密鍵、client credential、失効情報はProject内へ保存しない。
- Mobile credentialはplatform secure storageへ保存する。
- Panel、selection、scroll、表示中Conversationはclient固有状態として扱う。
- Conversation本文、Task、Execution、Workspaceの正規状態はbackendへ保存する。
- credentialや秘密情報をConversation snapshotへ含めない。

## 検証

Remote基盤は少なくとも次を自動検証します。

- major version不一致を拒否する。
- minor versionを共通範囲から選ぶ。
- one-time tokenを再利用できない。
- credential失効後に接続できない。
- 未付与capabilityのrequestを拒否する。
- absolute path、親参照、Workspace外symlinkを拒否する。
- stale Revisionのeditとsaveを拒否する。
- 切断後にTerminalを実行中と推測しない。
- 不正message、oversize message、認証前requestを拒否する。
- reconnect後にsnapshotでcacheを置き換える。

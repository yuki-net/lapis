# WebTransport (QUIC) 直接接続 — Phase 1 設計

Issue #365 の Phase 1（設計・仕様確定）。ブラウザーを `web-bridge` 経由の WebSocket ではなく、Host の QUIC endpoint へ WebTransport で直接接続させ、通信経路を Web / Desktop で統一する。

現行の通信（認証・request/response・event 連番・切断回復）は `web-bridge` が既に中継しているので、本設計は**トランスポートの載せ替えだけ**を行う。認証の新規設計（pairing / credential / OIDC / capability）は対象外とし、`docs/specs/auth.md` と `docs/specs/remote.md` の契約をそのまま使う。

## 目的と現状

| 役割 | 現状の実装 | 本設計後 |
| --- | --- | --- |
| Host の待受 | `remote::QuicServer`（quinn, ALPN `lapis.v0`, Client 起点の bidi stream 1 本）。`RemoteServer`/axum の WebSocket は未使用 | 同じ endpoint に WebTransport のサーバーセッションを追加する |
| Desktop | `sync_client::QuicConnection`（quinn + 自作 cert 検証） | 変更しない（同じ wire、同じ cert 検証） |
| Web の接続 | ページを `lapis-web` が配信し、`web-bridge` がブラウザーの WebSocket を Host の QUIC へ中継 | ブラウザーが `WebTransport` で Host に直接接続する |
| ページ配信 | `lapis-web`／`web-bridge --dist`（+ OIDC の `/auth/oidc/*`） | `lapis-web` が担う（OIDC は受け口を移す） |

`web-bridge` が担う処理のうち、値を生んでいるのは次の 3 つだけで、いずれも Protocol 層の話ではなくトランスポートである。

1. WebSocket の text frame と QUIC の長さ付き frame の相互変換（`host/transport/web-bridge/src/main.rs` の `forward`）。
2. Host 接続情報（`wire::HostConnection`）を接続ごとに読み直す追従。
3. 同一入口での Origin 照合と、`Session`/`Request`/`Event` の決まった流れ（認証 → request/response、event は Host 起点の push）を成立させるための双方向維持。

1 は WebTransport のストリームに置き換わり、2 はブラウザーが受け取る接続情報（後述の invite v2）に置き換わり、3 は Host 側が既に持っている Session 処理（`remote::server::handle_socket` に相当する部分）が担う。

## 採用する設計

### 1. トランスポートの対応付け（ストリーム多重化）

Protocol を変えないため、`ClientMessage` / `ServerMessage` の JSON とイベント連番の意味はそのまま。既存の「1 接続 = 1 本の Client 起点 bidi stream、各フレームは 4 byte big-endian 長 + UTF-8 JSON」を WebTransport では次のように対応させる。

- 1 WebTransport セッション = 1 認証済み Session（= Desktop の 1 QUIC 接続と同じ扱い）。
- ブラウザーが bidi stream を 1 本開き（`datagrams.createWritable()` ではなく `transport.createBidirectionalStream()`）、`ClientMessage` をクライアント→サーバー方向に、`ServerMessage` をサーバー→クライアント方向に書く。長さプレフィックスと JSON 形式は QUIC と同一にする。
- 認証後に `workspace.connect` / `workspace.close` が来たときの購読の張り替え・event 配信は、QUIC 用の `handle_socket` を共有する。transport 固有の分岐は「読み書き primitive」だけに閉じ込める。
- サーバー起点の bidi / uni stream と datagram は今回使わない。後で大量転送を流す必要が出たときに用途を追加する（下の「決めないこと」）。
- 空フレーム・サイズ超過・不正 JSON は、WebTransport でも 1 つの `PROTOCOL_ERROR` → 接続 close として扱う。

この対応付けの変更は、現在の `remote::RemoteSocket`（`socket.rs`）に `WebTransport` 変種を 1 つ増やし、`handle_socket` から見た `recv` / `send` の意味を変えないところまでに閉じ込める。`session.rs` / `backend_handler.rs` / `authorization.rs` / `auth.rs` は変更しない。

### 2. TLS 証明書戦略

ブラウザーの WebTransport（Chrome/Edge 系）は通常の CA 検証を行うが、`WebTransportOptions.serverCertificateHashes` を渡すと「ハッシュが一致する自己署名証明書」を信用する。制約は次のとおり。

- 証明書の有効期間が 14 日以内（`notAfter - notBefore` も含む）。
- 署名鍵は ECDSA（P-256 など）であること（SHA-256 hash が `serverCertificateHashes` で示される）。
- ハッシュは**リーフ証明書の DER 全体**の SHA-256。

採用案は **`serverCertificateHashes` + 短期の自己署名証明書**（正規証明書・ACME は採用しない）。

- 理由: 現行の接続情報は既に証明書 DER を署名経路（invite ファイル／メモリ）で渡しており、Desktop の cert 検証も「配布された証明書だけを root として信頼」している。同じ信頼モデルをブラウザーでも保持できる。正規証明書はドメイン取得・ACME 運用が新たに必要で、P2P（後述）と組み合わない。
- 期限: 7 日で生成する（Chrome の 14 日の上限より十分短い）。**同じ証明書のまま再起動を跨ぐ**ため、`--state-directory` の `<state>/identity` に保存した証明書の `notAfter` までの残存を持つ。残りが 24 時間未満になったら次回起動時に作り直す。
- `notBefore` を「生成時刻 - 1 時間」にずらす。ブラウザーとサーバーの時計ずれで `notBefore` 前に見えると拒否されるため。
- **証明書ローテーション**: 証明書は Host の身元と一体（`<state>/identity`）であり、WebTransport の 14 日制限は身元の寿命ではなく向き先を変えるものである。証明書だけを WebTransport 用に別に持つと Desktop / invite / profile の cert が 2 系統になり状態の所有者が割れるため採らない。代わりに、`HostIdentity` は `notAfter` が 24 時間未満のときだけ再起動時に新しい ECDSA 証明書を作り直し、`--certificate` の公開 DER も新しいものへ差し替える。Desktop の保存済み profile は `${cert}` の真正性で区別するだけで、identity そのもの（`host_id` と credential）は変わらない。証明書が変わった場合は、`host_id` が一致していれば Desktop は保存 profile の証明書を採用して再接続する（`devices` 承認はやり直さない）。`host_id` が一致しない場合だけ `HOST_MISMATCH` とする。この「host_id が一致する限り cert 差し替えを許す」規則は認証の変更ではなく、既存の `host_id` 照合の使い方であり、Phase 2 で `sync_client` と `auth.rs` の照合順序を「host_id → credential」に固定する。
- **署名鍵のアルゴリズム**: `rcgen 0.14` の `KeyPair::generate()` は aws-lc-rs の ECDSA P-256 を生成できる。現行 `HostIdentity::generate` は `rcgen::generate_simple_self_signed`（既定の鍵種）から作っているが、WebTransport では ECDSA を固定する。
- `serverCertificateHashes` は `window.isSecureContext` でのみ使える。`http://127.0.0.1`/`localhost` は secure context なので開発中の直近接続は成立する。Tailscale の `http://100.x…` は secure context でないため、ローカル WebTransport を `http://` だけで成立させることはできない。これが後述の「NAT 越え」と表裏になる。

### 3. NAT 越え・接続確立

方針は **ローカルは loopback で直結、遠隔は Tailscale（secure context が得られる経路）で直結、その他の NAT 越えは行わない**とする。ブラウザーから能動的に NAT を越える手段が無いため、NAT 越えの選択肢はこの 3 つに尽きる。

- **同じ端末（loopback）**: loopback（`127.0.0.1` / `localhost`）は secure context 扱いなので、`http://127.0.0.1:<port>` でも `serverCertificateHashes` が使え、WebTransport を直接開始できる。Host の QUIC 待受が loopback のとき、ページ配信（`lapis-web`）も loopback の同一または別ポートで行う。UDP の待受ポートは Host の QUIC と同一のまま（TCP の 4747 とは分かれたまま）。
- **Tailscale 経由（推奨の遠隔経路）**: `http://100.x…` は secure context にならないため、WebTransport を開始できない。`https://` での配信が必要になる。`tailscale cert`（MagicDNS 名）と `tailscale serve` でページを配信し、Host の QUIC 待受を tailnet の IP で直接公開すれば、WebTransport は `wss` 相当の `https://<MagicDNS 名>:<UDP port>` に直結できる。`tailscale serve` 自体（HTTP/1.1/2 の reverse proxy）は WebTransport を透過しないので、ページ配信だけ serve に乗せ、接続先は Host の UDP ポートにする。証明書は `serverCertificateHashes` で身元を固定するため、MagicDNS の CA 証明書を OS に信頼させる必要はない。serve を経由した 1 入口の配信は、Phase 1 では検証対象として残す（下の未検証）。
- **P2P UDP ホールパンチング**: 実装しない。ブラウザーから任意の UDP ポートへ最初のパケットを出す手段は無く、`new WebTransport()` は Host からの先出しパケットを trigger にできない（HTTP/3 CONNECT-UDP のエンドポイントが要るため）。Host から先に送っても、ブラウザー側のポートを決められない。Issue の「Host 側から Client へ先に UDP パケットを送って NAT に穴を開け、同じポートで `new WebTransport()` を受ける」は、ブラウザーが NAT の内外を問わず同一ローカルポートで QUIC を開始しないため、この形では成立しない。このため、Tailscale 以外の NAT 越えは今回のスコープ外とする（下の「決めないこと」）。
- **リレーフォールバック**: 今回入れない。リレーを足すとしても、Host 同様の Rust プロセスが QUIC で受ける WebTransport リレーであり、WebSocket への変換ではない。これは `host/gateway`（複数 Host の集約や TLS 終端）に近い役割になるため、必要になった時点で `docs/specs/architecture.md` の「gateway を作らない」の判断をやり直す。今回のスコープでは判断しない。

### 4. ページ配信と OIDC の受け口

`web-bridge` の削除後に `/auth/oidc/start` と `/auth/oidc/callback` を失わないため、次のどちらかに寄せる。

- **採用: `lapis-web`（`host/transport/web`）へ移す**。`host/transport/web-bridge/src/oidc.rs` は `OidcBroker` の責務が「IdP へのリダイレクト、code の交換、`postMessage` の準備」であり、どの待受でも同じ。`lapis-web` が `--dist` の配信を持つ既存の役割に合う。`OidcConfig` / `OidcError` の同名型の扱い（`check_architecture.py` の `DUPLICATE_PUBLIC_TYPES` に `web-bridge` が入っている）は、`web-bridge` 削除と同時にこの例外も削除する。
- 代替案: 専用の `lapis-web-auth` を作る。ただし待受を 2 つ持つことになり、Origin と Cookie の境界も増えるので採らない。

managed Host（`host/app/src/web_publisher.rs`）は `lapis-web` の子プロセスとして公開を継続する。`--listen` は今と同じで、`--managed` の ready 行と stdin 監視の挙動を `lapis-web` に移す（いまは web-bridge 側にある）。`HostWebStart` / `HostWebStatus` の wire と Desktop の設定 UI はそのまま。

### 5. 接続情報（invite v2）とページへ渡す値

ブラウザーは Host の待受アドレス・server_name・証明書ハッシュを受け取る必要がある。現行 `#/invite?token=<hex>` を拡張し、**fragment だけを使う**（サーバー log・Referer に載せない）。

```
#/invite?token=<64 hex>&host=<ip:port>&name=<server_name>&sha256=<64 hex>
```

- `sha256` は cert DER の SHA-256。Desktop は保存 profile から同じ値を作れる。
- `host` / `name` / `sha256` が揃っていない fragment は invalid として扱う。プレリリースなので、WebSocket 前提だった旧形式の招待は受け付けない。
- 招待を発行するのは Desktop（`issue_web_invitation`）と Host console。いずれも `HostWebStatus` から `address` を取れるように `HostWebStatus` へ `address` / `server_name` / `certificate_sha256` を足す（wire 追加）。
- ページ内で `serverCertificateHashes: [{ algorithm: "sha-256", value: <8 byte Uint8Array> }]` を `new WebTransport(url, options)` に渡す。

### 6. Web クライアントの実装境界

`clients/transport/src/browser.rs` の `BrowserConnection` を WebTransport 版に置き換える。役割は今と同じで、変わるのは Socket だけ。

- `web_sys::WebTransport` を使う（`--cfg=web_sys_unstable_apis` が必要）。Wasm ビルドは `clients/web/package.json` の `wasm` script と、`desktop-ui` の wasm target で cfg を付ける。
- 受信は `transport.incomingBidirectionalStreams` で bidi stream を受け、`ReadableStreamDefaultReader` から長さ付きフレームを読み、`handle_message`（`ClientCore::receive`）へ流す。今の `onmessage` と同じ順序保証を保つ。
- 送信は `WritableStreamDefaultWriter` に長さ付けで書く。`request()` の pending 照合は既存 `ClientCore` を再利用する。
- close/error は `BrowserConnection` の `fail()` と同じ扱いで、pending を失敗させる。
- `take_events` のイベント channel もそのまま。
- 再接続（`connect_inner` の再試行、`can_retry_pair` の挙動）と、`Workspace` 切替時の `workspace.close` → `workspace.connect` は変更しない。
- `hostWebSocketUrl`（`clients/web/src/invite.ts`）は削除する。代わりに、v2 の fragment が必ず持つ `host` / `name` / `sha256` から WebTransport の接続先と `serverCertificateHashes` を組み立てる純粋関数を置く。接続先のホスト部は `host`（IP）、SNI は `name` を使う。`new WebTransport(url)` は URL のホスト名で SNI を組み立てるため、証明書の SAN には `name` と `host` の両方を含める。
- OIDC の popup は `lapis-web` の入口へ向けたまま。

### 7. 削除・置き換えるもの

| 対象 | 扱い |
| --- | --- |
| `host/transport/web-bridge/` | 削除。`oidc` は `lapis-web` へ移す |
| `remote::RemoteServer` / axum WebSocket | 削除（未使用。`axum`/`axum-server` 依存を `remote` から外す） |
| `wire::REMOTE_WEBSOCKET_PATH` / `REMOTE_WEBSOCKET_PROTOCOL` / `WEB_BRIDGE_PORT` | 削除。`WEB_PAGE_PORT` は残す |
| `workbench/state/src/web_access.rs` の `bridge_port` / `bridge_url` / `websocket_scheme` | 削除し、ページ URL だけを表示する。設定 UI の `web-bridge-url` も削除 |
| `host/app/tests/managed_web_auth.rs` の `LAPIS_WEB_BRIDGE*` | 置き換え（下の検証） |
| `scripts/ci/paths.py` の `host/transport/web-bridge/` 規則 | 削除 |
| `scripts/dev-session` / `.run/Run Web (Bridge).run.xml` / `.cargo/config.toml` の `web-bridge` | `lapis-web` に置き換え |
| `clients/web/wasm/tests/bridge_smoke.py` / `extension_terminal_smoke.py` の bridge 起動 | WebTransport のブラウザー結合へ置き換え |

## 検証（Phase 2/3 に引き渡す結合テスト）

- **Host 結合（Rust, CI）**: `QuicServer` を WebTransport 対応にした後、h3 クライアント（`web-transport-quinn` 等の crate を test 専用に使う）から接続し、認証 → `workspace.list` → `workspace.connect` → snapshot → event が今の `QuicClient` と同じ結果になることを確認する。`host/app/tests/` に追加。
- **ブラウザー結合（CI, Linux か Windows の Chromium）**: `lapis-host` と `lapis-web` を起動し、invite v2 を組み立ててヘッドレス Chromium で WebTransport に接続し、`workspace.list` と event 1 件を確認する。`bridge_smoke.py` を置き換える。
- **認証の既存テスト**: 既存の `host/app/tests/quic.rs` のセッション・認可・失効のテストは、transport を `RemoteSocket` 差し替えで WebTransport のケースを 1 本足すだけで同じ Session 処理が通ることを確認する。
- ブラウザーの実装差（証明書 14 日制限・P-256 必須）を 1 本は通す。

### 未検証（Phase 2 の最初に確認する）

1. `tailscale serve` を経由した `https` 配信ページから、`tailnet IP:UDP ポート` の WebTransport へ直結できるか（mixed content / secure context の判定）。できない場合は loopback 以外の遠隔接続は Phase 3 のスコープ外に落とす。
2. `web_sys` の WebTransport で `incomingBidirectionalStreams` の `getReader()` と、`WritableStream` の writer を同時に使ったときの順序が `ClientCore` の期待どおりか。
3. `rcgen` の ECDSA P-256 自己署名と aws-lc-rs の相互運用（`serverCertificateHashes` の値が一致するか）。
4. ハッシュ計算（SHA-256）の利用先は Host 側（`sha2` は既存依存）。`wire::HostConnection` は v1 のまま据え置き、ブラウザー向けの `host`/`name`/`sha256` は invite fragment だけに載せる。この場合 `docs/specs/auth.md` と `docs/specs/remote.md` の接続情報の記述は更新不要。

## 決めないこと（今回のスコープ外）

- P2P UDP ホールパンチング（ブラウザーから任意 UDP へ最初の出発ができないため、今回の形では不可。必要になったら CONNECT-UDP か Tailscale 依存のどちらかを別 Issue で判断する）。
- NAT 越えのリレーフォールバックとその課金・帯域管理。今回は `lapis-web` 側に OIDC を移すだけで、リレーは作らない。
- サーバー起点 bidi/uni stream、WebTransport datagram による大容量転送（Document/Terminal の出力は今の JSON フレームのままで足りる）。
- 正規証明書 + ACME 運用。
- 認証方式（pairing / credential / OIDC / capability）の再設計。`docs/specs/auth.md` を変えない。
- WebTransport の 0-RTT 再開と、接続移行（`connection migration`）の明示制御。

## 段階分割（Phase 2/3/4 への渡し方）

1. Host 側: `RemoteSocket` に WebTransport 変種を追加。quinn endpoint を h3 のサーバーとしても動作させ、両方を同じ `EndpointState` に配線する。Phase 1 の「未検証 1〜4」を先に潰してから着手する。
2. 証明書: `HostIdentity` に短期有効の ECDSA 証明書、`HostWebStatus` に `address`/`server_name`/`certificate_sha256`、invite v2。
3. Web クライアント: `browser.rs` を `web_sys::WebTransport` に置き換え、`invite.ts` を v2 に合わせる。
4. `web-bridge` 削除、`lapis-web` へ OIDC と managed 公開を移設、`check_architecture.py` / `paths.py` / CI / `dev-session` を更新。
5. 結合テストを置き換え、`cargo clippy` / `scripts/check.py` / `check_architecture.py` を通す。

`docs/specs/architecture.md` は Phase 4 で更新する（「gateway を作らない」の根拠から `web-bridge` の文言を消す）。`docs/specs/remote.md` の QUIC 節は WebTransport のストリーム対応を追記する。本ファイルは Phase 4 完了時に `remote.md` へ畳み込むかどうかを判断する。

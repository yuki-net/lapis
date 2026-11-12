# Remote — internal constraints and design notes

この文書はRemoteとLocal QUIC接続の信頼境界・接続寿命を定義します。操作手順は `host/README.md`、実行時挙動と検証はコード・テストを正本とします。

## 信頼境界

- LAN内の接続元であっても信用しません。Remoteは利用者が明示的に有効化する設計とします。
- 認証前にWorkspace等の情報を返さず、認証後も操作ごとのcapabilityを実行側が確認します。
- Client由来のpathをそのままOSへ渡さず、Workspaceと検証済みの相対pathから解決し、親参照・symlink等による範囲外アクセスを拒否します。
- OSのpathを相対pathへ変換するときは、表現できない名前を別の文字や区切りに置き換えずエラーとします。
- Local所有者だけが持つ `host.manage` は、フォルダの登録とWorkspace間のDocument移動を許可します。登録時のみ利用者が指定した絶対pathを渡し、Hostが実在・正規化を検証します。通常のresource操作は相対pathとIDを使い、Remote招待にはこの管理権限を付与しません。所有者も各sessionの通常操作は接続Workspaceに限定します。
- Host所有者が明示的に発行したユーザー用招待には `workspace.switch` と `workspace.register` を付与でき、登録済みWorkspaceの一覧・切替と新しいフォルダの登録を許可します。これらはWorkspace間の文書移動やHost内部path取得を許可せず、通常のWorkspace限定招待には付与しません。`workspace.switch` を持つcredentialは特定Workspaceに固定せず、接続ごとに一つを選びます。通常のWorkspace限定credentialはpairing先へ固定します。
- Document・Terminalなどのresource identityをWorkspaceとsessionのattachmentへ関連付けます。同じWorkspaceを許可されたClientはsnapshotから再アタッチできます。別Workspaceや未認証sessionからの利用は拒否します。
- 認証情報、ファイル内容、Terminalの入出力を不用意にログへ記録しません。

## Server側で実装済みの制御

- `server.rs`・`quic.rs` と `tls.rs`: Remoteが有効化されていなければlistenしません。WebSocketとQUICはTLS 1.3のみを許可し、TLS early dataを無効にしています。認証・認可・イベント配信は同じSession処理を使用します。
- `auth.rs`: Pairing Tokenには有効期限があり、照合時に取り除くことで再使用を防ぎます。Remote credentialはClient・許可capability・期限に結び付き、失効させると関連sessionも無効になります。`workspace.switch` を持たないcredentialはWorkspaceにも固定し、`workspace.switch` を持つcredentialは接続ごとにWorkspaceを選べます。Client bindingは維持します。Workspaceを指定しなかった場合は固定先、無ければHostが解決するprimary Workspaceを使います。未登録のWorkspace、固定先との不一致、期限切れ、別Hostのcredentialは区別したerror codeで拒否します。
- `auth.rs`: 認証済みsessionの有効性と、requestが要求するWorkspace・capabilityを検証します。
  `workspace.connect` だけは `workspace.switch` を持つ Session に限り接続中と別の Workspace を
  指定できます。切替が成功した時点で Session の接続先を移し、以後の要求は移った先で認可します。
  `workspace.close` は attachment と購読だけを解除し、Session は維持します。
- 購読イベントを送る前にもsessionの失効・期限を確認します。失効したClientへ新しいDocumentやTerminalの内容を配信しません。
- 購読イベントでも、内容の読取りにcapabilityを要するものは権限のないsessionへ渡しません。配信自体を止めるとevent連番が欠落してClientが切断するため、同じ連番のまま内容だけを伏せた通知にします。

## QUICのwireと接続寿命

- ALPNは `lapis.v0`。1接続につき1本のClient起点bidirectional streamを使い、各方向で4 byte big-endianのpayload長に続けてUTF-8 JSONの `ClientMessage` / `ServerMessage` を送ります。
- 長さを検証してからpayloadを読み、受信の途中でイベント配信へ切り替わってもcodecに部分frameを保持します。message上限・認証timeout・idle timeout・送信timeoutと、endpointあたり128接続の上限を設けます。
- Clientは利用者が信頼して渡した公開証明書とServer名を検証し、不一致を拒否します。証明書検証の省略や0-RTTでの認証・更新は行いません。
- 切断時は認証sessionとattachmentのみを解除します。RuntimeのDocument・PTYはHostが所有し、再認証とWorkspace接続後にsnapshotから復元します。送信失敗後のstreamは再利用しません。
- Hostの秘密鍵と証明書は、`--state-directory` 指定時に `<state>/identity` へOSのユーザー保護で保存し、再起動後も同じ身元で待ち受けます。state directory未指定ではメモリ内に限ります。credential検証情報は state directory 指定時に保存し、再起動を跨ぐ実行資源の復元は実装していません。保存済みの同一性を読めない場合は作り直し、その場合だけ証明書が変わるため再接続にはペアリングを要します。
- Hostは任意で、待受開始後の接続情報（待受アドレス・server_name・Workspace ID・公開証明書DERと版）をJSONファイルへ書き出せます。これはtoken・秘密鍵を含まない非機密の受け渡しで、招待ファイルの代替ではありません。書くのは待受成功後だけ、消すのは正常終了時だけとし、書いたアドレスが自分の待受アドレスと一致する場合に限ります。接続情報を読むClientは、認証要件と証明書検証を緩めません。同じ待受成功後に招待ファイルも書き出せますが、こちらは1回限りのtokenを含む機密として扱い、他の利用者が読めない場所へだけ置きます。

Desktopは招待ファイルから証明書と一回限りのtokenを取り込み、以後は証明書とcredentialを保存したprofileを使います。Windowsではユーザー単位DPAPI、Unixの開発・CIではprivate directory/fileで保護します。Local所有者情報は親子の匿名pipeだけで受け渡し、引数やログへ秘密を出しません。Local所有者の鍵はOSユーザーとHostの寿命に紐付き、Remote credentialとは期限を分けます。

管理Local Hostは、起動した子プロセスのPIDとの一致と、Host自身が終了まで保持する`local-v1.lease`の排他ロックで起動と寿命を確認します。OSのprocess開始情報は取得できる場合のみ記録します。OSがロックを解放していれば、古いPIDの再利用先へアクセスできなくても起動し直せます。ロックが保持されている間は通信失敗だけで置換しません。ロック情報のない旧Local記録はPIDと開始情報で確認し、同一性を証明できない場合は誤置換を避けて明示的な再起動を案内します。

## Clientの回復

- `sync_client.rs` は機能workerから呼び出し、再接続では保存済みの証明書・credentialを検証し、Workspace接続とsnapshot取得でattachmentを復元します。送信済み更新要求は自動再送しません。期限を過ぎた待機要求は実行しません。
- Workspace を指定していない profile は、認証で Workspace を要求せず、Host が返す Workspace へ接続します。Host が返さない場合は resolver の primary Workspace、最後に `workspace.list` の先頭を使います。
- Host は認証応答で永続化した `host_id` を返せます。Web / Desktop は保存済み credential の `host_id` と照合し、別 Host への誤接続を区別したエラーにします。
- Web と Desktop は接続後に Workspace 名を表示します。接続中 Workspace の名前と root ディレクトリ名は `workspace.connect` 応答の identity を正本とし、表示名の解決に `workspace.list` は使いません。`workspace.list` の用途は切替 UI の候補と、profile の Workspace が未解決のときの選択 fallback だけです。Web は `workspace.switch` が付与されていればアプリ内の Workspace メニューから切り替えられます。切替は同じ接続のまま `workspace.close` で attachment を解除してから `workspace.connect` を送り、ページ遷移・再接続はしません。Workspace ごとに credential を複製しません。Web の state 層は、ページの生存中だけ Workspace ごとのクライアント複製を保持します。保持は接続中の Workspace 一覧数を上限とし、超えた分は古いものから破棄して次回は snapshot から復帰します。戻った Workspace は保持した複製を復元した後、`snapshot.resync`（Reconnect）と `workbench.get` で差分を再同期します。再同期に失敗した複製は設置せず保持へ戻し、切替先を閉じて元の Workspace へ接続し直します。保持の永続化はしません。
- Desktop は `workspace.connect` の identity を取得できなかった場合、root のディレクトリ名から表示名を推測しません。Remote の root は Host の実ディレクトリではなく profile の label から構成する仮想パスであり、推測した名前は接続先と一致しないためです。既に分かっている名前は保持し、無ければ Workspace ID だけを表示します。
- イベント購読のqueue上限を超えた場合は購読を切断し、snapshotから回復します。

認証拒否、protocol不一致、証明書不一致を単なる一時切断として自動的に信頼し直しません。

## 未実装事項

- QR・手動codeによるPairing招待の配布。
- Server秘密鍵・証明書のローテーションと失効手順。保存と再利用は `--state-directory` 指定時に実装済みです。

## 状態と回復

- 接続が失われても、実行Host上のTerminalやAgentが終了したとは推測しません。
- 再接続では権限と実行状態を再確認し、古いClient cacheを最新のsnapshotに整合させます。
- Documentの変更・保存はrevision等を使って競合を検出し、古い内容で黙って上書きしません。未送信の編集とHostの変更が競合した場合は、Hostの本文でローカル編集を置き換えず、外部変更として記録して復旧できる状態を保ちます。
- Terminalの画面・mode・端末問い合わせへの応答はHostのTerminal Engineが所有します。ClientはSnapshot / Deltaのrevisionを検証し、欠落時には最新Snapshotへ再同期します。raw VT/ANSIやbyte offsetをClientへ公開しません。
- Messageサイズ、request数、session数、timeoutなど、必要な資源上限を設けます。

## 最低限の確認項目

未認証・権限不足・Workspace外path・失効済credentialを拒否できるか、stale revisionを検出できるか、切断中の実行状態を誤認しないか、再接続時にcacheを更新できるかを検証します。実行可能な検証手段の正本はテストコードとCIです。

## イベント駆動の同期

Client は定期ポーリングで状態を取りに行かず、Host のイベントを契機に更新する。

- Workbench は `workbench.changed`、Terminal は `terminal.delta` / `terminal.status`、Document は `document.*` を購読する。
- 外部ファイル変更は Host の Workspace 監視から `files.changed`、Task の変更は `task.changed` として publish する。どちらも変化の通知だけを運び、状態の正本は各 snapshot とする。
- 購読は `DocumentConnection::subscribe` / `TaskConnection::subscribe` のように Adapter が通知 channel を返す。購読できない接続では初回取得だけを行う。
- 通知の欠落・購読の切断時は Snapshot と revision で再同期し、購読を張り直す。再接続待ちの短い backoff はタイマーで持ってよいが、通常時の定期 refresh は行わない。
- Workbench の表示変更はローカル保存の debounce を待たずに Host へ通知し、同じ Workspace の購読者へ反映する。ディスク保存は従来どおり debounce し、反映と永続化を分離する。
- 未確定の操作（編集中・送信中）がある間は取得結果を保留し、完了時に 1 回だけ反映する。未保存の編集を通知で上書きしない。

## Workbench のタブ参照

Workbench に保存するタブは `TabRef` で参照先だけを保持する。Document は `DocumentId`、
Terminal は Host の `TerminalId`、その他の View は `ViewInstanceId` を識別子とする。
復元した Client は参照先の Resource に再アタッチし、Terminal では新しい profile を開始しない。
保存済みの旧形式（文字列の View ID）は読み込み時に `TabRef` へ変換する。

## Workspace の view state 共有

Workspace を開いている Client 間で、開いているタブと選択中の Document を自動共有する。
真実は Host が Workspace 単位で保持し、Client は参加時の snapshot を取り、以後の変更を
delta として受け取る。protocol 0.13 で追加した契約で、Desktop（PR #314）と Web の
両方が購読する。

- `workspace.view.subscribe` は共有へ参加し、`WorkspaceViewSnapshot`（`revision` と
  `Option<WorkspaceView>`）を返す。まだ誰も共有していない Workspace は `view` が `null`。
- `workspace.view.update` は `WorkspaceView` 全体を送り、Host は同時更新を直列化して
  受理した `revision` を返す。マージはせず、最後に受理した値がそのまま正本になる
  (last-writer-wins)。`expected_revision` による競合検出はしない。
- Host は受理した値を `workspace.view_changed`（`revision` と `view`）として配る。
  これは差分計算の結果ではなく置き換えるべき最新値であり、Client は丸ごと適用する。
- `workspace.view.unsubscribe` は共有から抜ける。Workspace を閉じた Session は
  `workspace.close` で attachment と購読を解除し、以後の delta を受け取らない。
  `workspace.close` は Session を終了させない。同じ接続のまま `workspace.connect` で
  再接続・切替できる。Session の終了は接続断と失効だけが行う。
- 共有に参加している Session が無い間は `workspace.view_changed` を publish しない。
  誰も見ていない更新で workspace の event 連番を消費しないため。
- Host は Workspace を閉じても最後に受理した view state を保持し、次に参加した Client へ
  snapshot として返す。永続化はせず、Host の終了で失う。
- `WorkspaceView` は `clients/state` の `WorkspaceViewState` のうち、Client 固有の
  `root`（絶対 path）と未保存内容（`draft_content`）を除いた形。Document の内容は
  保存済みのものだけを既存の Workbench 同期・Document API 経由で共有し、未保存 buffer の
  リアルタイム共有はこの契約に含めない。
- 各文書の ID は `WorkspaceDocumentView.document_id`、選択中の Document の ID は
  `WorkspaceView.active_document_id`。どちらも domain の `editor_core::DocumentId` で、
  wire の `client_api::DocumentId` とは別型。変換は `From` / `TryFrom` を使う。
- Document ID は Client の複製キーである場合がある（Web の `web-N` は Host の ID と
  一致しない）。ID を共有できない Client は `document_id` と `active_document_id` を
  `null` にし、Workspace 相対 path だけでタブ構成とアクティブ文書を伝える。適用側は
  ID が無ければ path で文書を解決する。
- 購読は Session 単位で受けるが、event は同じ Workspace に接続している全 Session へ
  同じ連番で配る。`clients/transport` は event 連番の欠落を切断として扱うため、
  Session ごとに配信自体を絞ると未購読 Session がずれを検出して切断される。
- 内容は読取り権限で絞る。`workspace.sync` を持たない Session へは `view` を `null` に
  した `workspace.view_changed` を届け、文書 path とカーソル位置を渡さない。絞り込みは
  Remote の配信境界（`host/transport/remote/src/backend_handler.rs` の event sink）で
  行い、Runtime は全 Session へ同じ event を配る。Client は自ら購読していない間と
  `view` が `null` の `workspace.view_changed` を適用しない。
- パネル構成とレイアウト寸法、開いているタブ一覧は既存の Workbench 同期
  (`ConversationViewState.layout`) がすでに共有しており、この契約はそれを置き換えない。
  `for_shared_workbench` がアクティブタブを共有から外す挙動も変えない。Client 統合では
  両者を合成して表示する。
- Desktop は Workspace を開いた時点で購読し、snapshot を `WorkspaceViewState` に適用して
  以後の delta も反映する。Workspace を閉じたときと終了時に購読を解除する。
- Desktop が共有へ送るのは開いているタブの並びとアクティブ文書だけで、カーソル位置・
  選択・スクロールとパネル寸法は送らない。送信は 150ms の debounce を挟み、直前に
  適用したタブ構成と同じ間は再送しない。
- Desktop はウィンドウが再びアクティブになったとき（スリープ復帰やバックグラウンド復帰）
  に snapshot を取り直して再同期する。受信の異常や event の欠落時も snapshot へ戻る。
- Web は Workspace へ接続した時点で購読し、snapshot と delta をタブ構成へ適用する。
  送信は Desktop と同じく 150ms の debounce と直前のタブ構成との比較で抑え、
  `revision` / 自己通知の `sequence` で自分の更新を再適用しない。`view` が `null` の
  event は無視する。
- Web はページがアクティブな間だけ live とする。タブのバックグラウンド復帰と再接続時は
  snapshot で再同期し、切断中は最後に受信した値のまま古い可能性を表示する。ページ終了時と
  Workspace を閉じたときは `workspace.view.unsubscribe` を送る。
- Web の `WorkspaceView` は Client 固有の `root` を持たないため、仮想 root からの相対 path
  で送受信する。文書の内容は `document.save` の後に Host の正本を取り直して追従する。
- 共有 view の適用ではタブの並びと選択中の文書だけを変更し、既に開いている文書の内容は
  置き換えない。共有側で閉じられた未保存の文書は、データ損失を避けるためタブとして残す。
- 共有するのはタブ構成と文書の更新状況だけで、レイアウト寸法やレスポンシブな状態は
  各端末に委ねる（Issue #290 のコメント）。

## Terminal の同期契約

Terminal の正規状態は Host が所有する。PTY の raw bytes は Host 内部の
Terminal Engine にだけ渡し、Client へは `TerminalSnapshot` と
`TerminalDelta` で構成した Lapis Terminal Model を送る。Client は raw VT/ANSI
を解釈せず、受信した screen、cursor、mode、revision を表示用 cache に適用する。

- 初回 attach、再接続、revision gap、状態不一致では Snapshot を使う。
- 通常更新は連続した `TerminalDelta.revision` の changed rows を使う。
- `terminal.input` / `terminal.resize` / `terminal.terminate` の応答は受理だけを返し、
  画面や状態は載せない。状態は `terminal.delta` / `terminal.status` イベントで届く。
- 入力と resize は操作権を持つ Session だけが行える。`terminal.claim` で取得し、
  `terminal.release` または接続断で解放する。操作権の変更は `terminal.owner` で通知する。
  `TerminalSnapshot.owner` も現在の操作権を表すため、Client は Snapshot からの復旧時にも
  owner を反映し、通知が欠けた場合でも表示を Host と一致させる。
- Client から Host へは `TerminalInput::Text`、`Key`、`Paste` を送り、PTY bytes
  への変換と bracketed paste の処理は Host の Terminal Engine 境界で行う。
- Host は `terminal.list_profiles` で起動可能な `TerminalProfile`（program、args、env）を
  公開し、`terminal.start` は raw shell ではなく profile ID を受け取る。 Client は一覧に
  ない profile ID を実行できず、未指定時は Host の既定 profile を使う。 Local も Remote も
  同じ Host Runtime と Engine を通るため、IME の確定 text、bracketed paste、scrollback の
  上限と差分再同期は同じ契約になる。
- attach、表示、focus、別 Client の接続では PTY size を変更しない。Client の
  panel 幅に追従した resize など、明示的な resize 操作だけが canonical size と
  Engine size を更新し、全 Client に通知する。
- Client の切断、Window の close、画面の切替は Terminal process を終了させない。
  `terminal.terminate` を送るのは Terminal タブを閉じる操作と、右クリックメニューで
  明示的に「終了」を選ぶ操作で、いずれも Host の PTY を終了させる。
  Client はこのとき表示中の Terminal を cache から除き、再アタッチの対象に戻さない。

この契約により、Desktop は GPUI Renderer、将来の Mobile は native Renderer として
同じ Model を描画できる。Agent Session、ACP、Chat UI、provider 固有の handoff は
Terminal の同期契約に含めない。

Scrollback は Snapshot では bounded な履歴全体を含め、Delta では追加行と削除数を
送る。履歴の再構成が必要な resize、alternate screen 切替、状態不一致では reset
Delta または Snapshot を使い、Client 側で履歴が無制限に増えないようにする。

## Document の編集権

Document の内容と revision の正規状態は Host が所有する。編集権は Session 単位の
owner として Host が保持し、`DocumentSnapshot.owner` と `document.owner` イベントで
Client に通知する。

- `document.edit` / `document.undo` / `document.redo` / `document.discard` は owner の
  Session だけが実行できる。owner が未設定の Document は従来どおり編集できる。
- `document.claim` は編集権を取得する。他の Session が所有中なら所有権を移す。
  `document.release` は owner 自身だけが実行できる。
- 接続断と無操作タイムアウト（既定 300 秒）で Host は owner を解放し、
  `document.owner`（owner なし）を通知する。
- 編集権の変更は revision を進めない。Client は owner が他の Session の間、
  Editor を読み取り専用にする。

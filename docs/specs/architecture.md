# 責務と依存方向

機能を追加・移行する開発者が、状態の所有者と配置先を判断するための正本。

scripts/check_architecture.py は crate 間の依存方向に加え、Desktop Layout の旧識別子、Feature からの viewport / user-agent / file_dialog 参照、Feature のスクロール直書き、crate をまたぐ同名の公開型を検査する。同名型を新たに増やす場合は重複として拒否される。

| 領域 | 所有するもの | 所有しないもの |
| --- | --- | --- |
| `clients/` | Client の Window、Panel、Tab、focus、selection、表示 cache | Host 上の実行資源 |
| `contracts/domain/` | Transport 非依存の値・ドメイン規則・port | GPUI、通信、外部プロセスの具体実装 |
| `contracts/protocol/` | request/response/event、revision、認可可能な操作契約 | OS 実装、Client の画面構成 |
| `contracts/transport/wire/` | Transport 固有の wire DTO | domain の規則、Client の画面構成 |
| `extensions/git/contract/` | Git 拡張の wire 契約（domain git の DTO を再エクスポート） | 拡張の実装、Client の画面構成 |
| `clients/transport/` | Client共通の非同期要求・応答照合と接続世代 | UI、具体的な通信経路 |
| `host/application/` | 操作と判断、port による外部連携 | GPUI、OS 固有 API |
| `host/runtime/` | 正規状態、操作の直列化、購読・revision・実行資源の寿命 | Window、Panel、focus |
| `host/workbench/` | Workbench の wire 境界・容量検証・永続化 port | 画面描画、Workspace の正規状態全体 |
| `host/terminal/` | Terminal の正規状態・画面差分・接続 session | PTY の具体実装、画面描画 |
| `host/documents/` | Document 資源・未保存状態・snapshot | ファイル I/O、Workspace の正規状態全体 |
| `host/adapters/` | FS、PTY、LSP、runner、検索、永続化 | 画面の判断 |
| `host/extensions/git/` | Git 拡張の Host 実装（CLI 操作・監視） | 画面の判断、Workspace の正規状態 |
| `host/transport/` | 認証・wire・接続と共通 Runtime への配送 | 独立した Workspace/Document 正本 |
| `host/app/` | Runtime と具体実装の組み立て、開始・終了 | UI |

## 配置の判断

状態とリソースの所有原則は [全体アーキテクチャ](../ARCHITECTURE.md) を参照する。追加実装の具体的な配置は次のとおり。

1. 値・規則・port は models、通信に公開する契約は protocol、操作判断は application。
2. 外部環境に触れる具体実装は infrastructure。認証や wire は transport。
3. 実際の所有・寿命の境界がなければモジュールで分割する。空 crate や転送だけの層を作らない。

### `contracts/domain` の集約方針

- ID は `contracts/domain/editor-core` に集約する（`ProjectId` / `WorkspaceId` / `ConversationId` / `DocumentId` / `TaskId` / `ExecutionId`）。複数機能が同じ ID を参照するため、機能別 crate へ分散させない。
- 状態・値・エラーは各機能の domain crate（`document` / `git` / `task` / `terminal` / `workspace` / `workbench` / `lsp` / `settings` など）が所有する。機能をまたぐ移動は、複数 crate が同じ型を必要とする場合だけ行う。
- port（`WorkspaceRepository` / `TerminalBackend` / `GitBackend` / `TaskBackend` / `LanguageServerBackend` / `WorkspaceSearchBackend` / `DocumentRepository` / `WorkbenchConnection` / `SettingsRepository`）は、実装ではなく利用側の domain crate に置く。実装は `host/adapters/*` と `host/extensions/*` が持つ。
- 追加の集約は行わない。現状の配置で「どの crate が正本か」が一意になっており、統合すると domain 間の依存が増えるだけで判断材料が増えない。

### `host/application` を独立させない

- 操作の判断は `host/runtime`（`state` / `operations` / `state/*`）が持ち、`runtime` を application とみなす。`host/application` は作らない。
- 理由: 判断と正規状態が同じ lock と寿命を共有しており、crate を分けると状態の受け渡しだけの層が増える。実際の境界は `host/workbench` / `host/terminal` / `host/documents` / `host/workspaces` として既に切り出している。
- 将来、外部 I/O を伴わない純粋な判断が増え、`runtime` から独立した寿命を持つ場合だけ再検討する。

### Runtime を分割しない

- `host/runtime` は Workspace / Session / Event / revision を1つの正本として持ち続ける。Document・Terminal・Workbench は状態と request 処理を `host/*` と `state/*` に分けたが、正規状態の所有者は Runtime のままとする。
- 理由: request の直列化、event 連番、revision 比較が同一の正本に依存しており、プロセス内で分割すると整合性の担保が通信境界へ移る。

### `host/gateway` を作らない

- Web は Host と同一プロセスに同梱せず、Client が Host へ直接接続する現行構成を維持する。`host/gateway` は作らない。
- 理由: gateway が必要になるのは複数 Host の集約やブラウザーからの認証終端を分離する場合であり、現時点の Web は `web-bridge` と Remote transport で要件を満たしている。
- 複数 Host の集約や TLS 終端の分離が必要になった時点で、`host/gateway` の新設を別途判断する。

## Runtime と接続の契約

- Local Host実行ファイルの選択はDesktopのComposition Rootが行う。既定はDesktopと同じ出力先とし、`--local-host-executable` の明示指定はLocal接続へ注入する。開発launcherがビルドを準備し、UIやBackendに開発ツールの出力先探索を持ち込まない。
- Host が登録した Workspace ごとに Runtime が Document の内容・revision・undo/redo、Terminal の出力・状態、Git/Task の操作範囲を所有する。Client の Document は表示 cache であり、編集は revision を指定して正本へ送る。
- 埋め込み接続と Remote は同じ request/response/event と Runtime を使う。Transport は認可と配送を担い、別の正規状態を作らない。
- Git Repository の status・revision・監視はHost Runtimeが所有する。Git操作と外部変更を同じ更新経路へ集約し、変更を既存のWorkspace eventとsnapshotでClientへ配信する。監視に失敗した場合はHost側で低頻度に再確認し、監視を再試行する。
- Git/Task の外部 I/O は正規状態 worker の外で実行する。Runtime 内で接続・Workspace を検証し、Workspace ごとの実行資源を直列化したうえで、結果とイベントを worker に戻す。他 Workspace の操作や Document・Terminal の更新を Git の完了待ちにしない。
- Git/Worktree/Task の公開操作は Workspace・Worktree・Execution の ID を指定する。実行ディレクトリ、内部 TaskRecord、OS の絶対パスを wire に公開せず、実行 ID と初期状態は Runtime が生成する。
- Document の close や接続切断は attachment の解除であり、未保存内容・履歴・Terminal・Agent の終了ではない。実行終了には明示的な終了操作が必要。
- Document の古い revision は拒否する。保存済みの Client 表示状態を復元しても、すでに存在する正規 Document の新しい内容を上書きしない。正規内容と復旧用 draft が異なる場合は、draft を別の未保存文書として保持する。
- Workspaceの共有Workbench表示状態はHostのRuntimeが所有する。更新時は取得済みrevisionを指定し、不一致なら現在値を返して変更しない。更新通知は同じWorkspaceの購読者へ、Clientのローカル保存完了を待たずに配信する。未登録状態と既定表示を区別し、書き込みには専用capabilityを要求する。管理Hostと保存先を指定した独立Hostでは状態を永続化する。
- 状態ディレクトリを持つHostは登録済みWorkspaceのIDとHost内pathを保持し、再起動時に復元する。利用できないpathや識別が一致しない記録は消去せず、該当Workspaceを復元できないことを通知する。Host内pathはWorkspace一覧のwire応答へ含めない。
- 共有Workbenchの文書表示状態はWorkspace ID、Workspace相対path、文書ID、表示位置と復旧draftで表し、Client固有の絶対pathを含めない。Hostは別Workspaceの状態を拒否する。旧形式からの変換に失敗した場合は元の保存値を保持する。
- 旧ClientがWorkbench更新で文書表示状態を省略した場合、Hostは保存済みの文書状態を保持する。新Clientは変更しない値も含めて明示し、`null`を指定した場合だけ文書状態を消去する。
- WorkbenchのClient側状態遷移は同期・非同期の通信方式で共通とする。送信失敗、処理の中断、競合、不正な更新応答では未確定値を保持し、競合時のHost状態と分けて扱う。Hostの状態を採用して未確定値を破棄する操作と、ローカル値の再送は明示的に行う。
- 送信応答が失われた後に再取得した正本が未確定値と完全に一致し、確定済みrevisionの直後である場合は、その値を確定する。それ以外の新しい正本は競合として保持し、未確定値を上書きしない。
- Workspace を開いている Client 間で共有する view state（開いている Document と表示位置）は、Workbench の revision 付き更新とは別の Session 単位 channel とする。Host の Workspace 状態が正本で、同時更新を直列化して last-writer-wins で置き換え、マージはしない。参加した Client は snapshot を取り、以後は置き換え用の最新値を受ける。参加者がいない間は event を発行せず、Workspace を閉じた Session へは配らない。
- 重なる Workspace grant から同じ canonical path を別の Document として開く操作は競合として拒否する。別 Workspace の内容を暗黙に共有して認可範囲を広げない。
- Terminal の購読欠落は snapshot と sequence で回復する。一時的な poll・通信失敗を実行終了とみなさず、出力の置換 snapshot と追記イベントを区別する。

## Resource Manager（管理下プロセスの計測）

- 対象は Host / Desktop が自ら spawn した root PID と、その PID から辿れる子孫だけとする。OS 全体の一覧や、Lapis 外で起動された同名プロセスを列挙・終了しない。
- Host と Desktop はそれぞれプロセスレジストリをローカルに持つ。Host のレジストリは Host 自身と Host が起動した Terminal / Task Run などの root を、Desktop のレジストリは Desktop 自身と Desktop が起動した Language Server などの root を所有する。Remote Host から Desktop 側の root は操作できない。
- Task / Execution のモデルと OS Process は独立させ、レジストリは root ごとに Workspace と所有元（Terminal / Task Run / Language Server 等）を保持する。Terminal 内の Agent CLI は Terminal の子孫として扱う。
- PID 再利用は、登録時と比較時で同じ開始識別子を比べて判定する。識別子は壁時計の変更で値が変わらないもの（Linux は boot 相対の開始 ticks、Windows / macOS は OS が記録した作成時刻）を使い、開始時刻を確定できないプロセスは所有対象にしない。表示用の開始時刻と同一性判定は別の値として扱う。
- 計測は 1 秒間隔で行い、root ごとに直近 60 サンプル（60 秒分）を保持する。CPU は 100 分の 1 パーセント、メモリは byte の整数として wire に載せる。終了した root は状態確認のため 60 秒残してから削除する。
- 終了操作は接続中の Workspace に属する root だけを対象とし、root とその子孫を OS のプロセスツリー単位で終了する。Host / Desktop 自身は終了対象にしない。所有外の root ID は存在の有無を区別せず拒否する。
- `workspace.execute` の `Processes` / `TerminateProcess` を使い、Local / Remote で同じ意味にする。読み取りは `workspace.process.read`、終了は `workspace.process.control` を要求する。
- Desktop の一覧取得は Resource Manager 専用の worker で行い、Document の編集・保存と直列化を共有しない。Workspace は worker 側が保持し、切替時は Feature から同期する。
- 一覧の再取得は Resource Manager パネルが表示中のときだけ 2 秒間隔で行う。非表示にしたら周期を止め、再表示時に 1 回取得してから再開する。

## 設定の所有境界

- 設定値の所有者は `User` / `Host` / `ProjectShared` / `ProjectLocal` のスコープで表し、項目ごとに保存を許すスコープは `contracts/domain/settings` の定義データが決める。優先順位・保存形式・UI の組み立て規則は [設定のスコープと所有者](settings.md) を正本とする。
- 共有 state と解決済みの実効値は `clients/state` が所有する。Feature は解決結果だけを読み、Feature ごとに設定 cache を作らない。
- 設定の保存は Feature の描画・入力処理から直接行わず、`SettingsCoordinator` の worker へ渡す。

## Desktop の更新境界

- Root は Entity の組み立てと機能間イベントの接続を担当する。Shell は Window・Panel・Tab・Layout を所有し、各 Feature の Backend 操作や表示 cache を集約しない。
- Feature は自身の入力・表示 cache・更新周期を所有する。共通 Client Adapter の worker で操作を直列化し、描画・入力処理から FS・Git・PTY や同期 port を直接呼び出さない。
- Terminal 入力は端末ごとの送信 queue で Text / Key / Paste の順序を保つ。Text は 1 frame 程度まとめて送り、Key / Paste は先行 Text を送ってから即時送信する。失敗時は受理済み入力を再送せず、端末状態を再同期して未受理分だけを再試行する。
- 応答の適用には要求順序と対象の世代を検証する。楽観編集の先行操作が失敗したら、数値 revision が偶然一致しても依存する後続編集を適用しない。未確定入力は明示的な復旧まで保持する。
- Window 終了時は受付済み入力と表示状態の保存を完了させる。購読・表示 worker の破棄によって Host の Terminal・Agent を終了しない。保存先や Workspace が失われても、復元用 draft を無言で破棄しない。

## Desktop の WindowGroup

DebugのInspectorでは、GPUIが描画要素の識別・検査snapshot・Hit Testingと検査状態を所有する。
Lapisのdevtoolsは独立OS Window・購読の寿命、Treeの開閉/表示cache、Overlayの配色を所有する。
Targetの世代付きWindowIdごとに一つのsessionを持ち、検査EntityIdで遅延UI操作を照合する。
Inspectorのcloseは対象を閉じず、Targetのcloseは対応するInspectorだけを閉じる。
Treeの可視行はGPUIのuniform_listで描画し、選択・Hover・開閉・scrollを他sessionと共有しない。
Window optionsはapp、タイトルバー/controlsはwindow_host::chromeをRoot・Detachedと共用する。
本文は通常Panelと共通のアイランド枠・ヘッダーを使い、Refresh/PickはDetails内に置く。
描画完了のEntity通知からInspectorだけを更新し、終了時に購読と対象の検査資源を解放する。
Document/Toolのタブ配置へInspectorを登録しない。公式との差分と撤去条件は
[vendor GPUI](../../vendor/gpui/README.LAPIS.md)で管理する。

- Root ごとに独立した WindowGroup と Client worker を持つ。Detached は元の worker・Document・Conversation を共有し、Feature や実行サービスを作り直さない。別 Root のグループへタブを渡さない。
- 共通の OS Window options と Shell が、テーマ・タイトルバー・Window Controls・タブ・フッターを描画する。Detached は Root 用のメニューや補助 Panel を持たない。
- WindowGroup は Window の親子関係、`PanelTab` の所属Window・`PanelId`、終了中フラグを持つ。同じZoneの複数Panelを`ZonePosition`で代用しない。再ドックは移動先WindowのMain Panelを解決し共通APIを呼ぶ。Window bounds はイベントで更新する。Active Tab・focus・selection・scroll・Panel サイズは各 Window の Shell / Editor が所有する。
- Document の操作は共有 worker で直列化し、送信元 Window の対象を操作前に選ぶ。新規 Document の作成元も worker 内で記録し、先に届いた Snapshot の送信元から所属を推測しない。応答の連番と各 Editor の世代で古い結果を拒否する。移動中に入力を失わないため、未完了入力・復旧待ち・終了処理中の移動は受け付けない。
- Worker 内の対象切替は `select_document_for_operation` を使い、表示状態を保存しない。Root の既存 poll と子の操作が異なる文書を対象にしても、切替だけで保存済み Active Tab を書き換えない。ユーザーによるタブ選択、文書変更、debounce 後の表示状態更新はそれぞれの保存経路を維持する。
- Window の生成成功後にタブの所属を移す。生成失敗・未受理のドロップでは元のタブを残す。新 Host の購読が有効になる前に初回 Snapshot と配置通知が併合される場合も、defer した通知で配置を渡す。
- close は入力の受付停止 → FIFO 上の入力失敗検査 → 全対象の保存/破棄判断 → 保存 → revision の事前検証 → タブの close → 表示状態の保存 → OS Window の閉鎖の順。Cancel / Save As のキャンセルではタブを閉じない。通常の Root close はその系列だけ、Quit は全 Root 系列を対象とし、判断が揃う前に一部だけ閉じない。複数Rootが同じDocument正本を開いている場合、判断・保存・破棄は一度だけ行い、各attachmentを個別に閉じる。外部 I/O の commit 失敗まで rollback できる分散トランザクションではない。
- Detached の終了はタブを閉じ、Root へ戻さない。表示 Entity・購読の解放と Host 上のプロセス終了は別操作である。
- 一括closeは確認済みの閉鎖ID、失敗対象、未実行ID、失敗段階を返す。表示状態保存とSnapshot取得はclose結果と独立させ、後段の失敗で成功済みattachment解除を隠さない。Clientが保持するattachment一覧と正本から更新したcacheを全対象WindowGroupへ反映してから終了中フラグを解除する。途中失敗ではOS Windowを閉じず、閉鎖済みタブ・所有情報だけを除去する。
- 正本の再取得が失敗しても、Clientの確認済み所属と最後のcacheをsequence付きで配信する。未確認cacheでの編集・移動・保存は止め、再度Close / Quitを実行したときのworker上の再照合で解除する。通常の遅延Snapshotでは復旧待ちを解除しない。残すDocumentは再attachして最新revisionと保存判断を確認し、応答不明のcloseやdiscardを成功と推測しない。DocumentCloseは正本文書の削除ではなく接続のattachment解除であり、再attachは残す意図を再確立する。worker自体が終了して再照合できない場合は、Windowとcacheを保持したまま操作を停止する。
- Editorの処理待ちでGroup Snapshotの適用を見送った場合は、古い世代の応答も含めて待ち処理が解消した時点でHostへ通知し、Groupの最新状態を再適用する。Shellは個別の遅延応答を直接表示せず、sequence検証後のGroup状態から更新する。

Windows では Document のタブを Window 外で離すと新 Window を作り、同じグループの Panel へ離すと再ドックする。Root / 子 / 子同士を同じ移動経路で扱い、移動先で文書を選択する。空になった子 Window は開いたままとし、閉じる操作で解放する。配置モデルは Document / Tool 共通だが、Tool の Window 間移動は未有効。Terminal は focus と入力先・出力購読、Task / Agent は選択 execution とストリームの寿命を持つため、既存 Entity を再利用して再購読範囲を検証するまで移動させない。個別の Window Manager や汎用 Trait は増やさない。

GPUI 0.2.2 の Windows backend は MouseDown で `SetCapture`、MouseUp で `ReleaseCapture` し、GPUI は受信した Window の MouseUp 後に active drag を破棄する。Shell は元 Window の capture phase で解放を受け、`ClientToScreen` / `ScreenToClient` で各 Window の DPI をまたぐ物理座標を変換する。`WindowFromPoint` の手前の Window と描画時の Panel bounds を照合し、背後の Window へ移動しない。別グループの Lapis Window と、Lapis の Panel 以外の領域では取り消す。外部アプリ上では新 Window を生成する。新 Window は離した位置のモニターの作業領域内へ置く。GPUI の変更やイベントの定期監視は加えない。

ドラッグ開始時には配置を変更しない。Escape は GPUI の key binding より前に処理し、focus を失った場合も取り消す。取消後の MouseUp、古い所属、終了中・入力処理中、Tool の Window 間移動は受理しない。同一Window内の移動先はOverlayで強調する。Windowsではタブのサイズと位置に合わせた追従プレビューを表示する。別Windowの移動先の強調表示は未実装。Windows 以外では Window 内のドラッグに対応し、Window 間ドラッグは未対応。

WindowGroup の再起動復元は未実装。後続では group / window の永続 ID、親子関係、bounds、Panel ごとのタブ順・Active Tab・サイズを、Document の復元用 ID と関連づけて保存する。現状の HostId / GPUI EntityId を永続 ID に使わない。明示 close 後の更新済み Document 一覧を保存し、閉じたタブ・Window を復活させない。既存の文書復元は OS Window の配置復元を意味しない。

`scripts/test_window_host.ps1` は隔離した Workspace と設定で Native Window の生成・共有編集・再ドック・グループ分離・終了・再生成と CPU の短時間サンプルを検証する。Windows では自プロセスのテスト Window だけに Mouse / Key メッセージを送り、Escape、Window 外への取り外し、重なった Window の再ドック、子同士の移動、別グループへの拒否、文書領域の高さも確認する。対象 Window の表示と重なり順はこの明示的な smoke モードだけで操作する。保存確認ダイアログ・IME・複数 DPI のモニターをまたぐ実操作と継続性能は別途確認する。

## Desktop の可変 Layout

- `DockLayout`はLeft・Main・Right・BottomのZoneとZoneの配置・表示を所有する。各ZoneはPanelの順序、比率、開閉状態を持ち、PanelはTabの順序と選択を持つ。Panelの所属ZoneはZone側で管理し、Panelには重複保持しない。PanelやZoneはDocument・Terminal・Agent等のResourceを所有しない。
- `ZonePolicy`はZoneの開閉・Panel数・成長方向・サイズ制約を定義する。Mainは常に開き、削除不能な一つのPanelを持つ。Zone内部の順序と比率は`PanelLayout`で表し、初期状態もPanel追加後も同じRendererを使う。空ZoneはTool Pickerから初回Panelを作成する。
- `DockGeometry`はViewportとLayoutからFrame・Zone・Panel・境界を計算し、描画と操作に使用する。RootとDetachedは同じFrameを使う。Zone操作は共通Commandを経由する。
- `ViewId`は種類、`ViewInstanceId`は個体を識別する。Layoutの保存値はZone・Panel・Tab参照だけを含む。旧Layout形式は復元せず、Task・未保存Documentなど別の保存値は保持する。
- 復元時はZone・Panel IDとTab個体の一意性、所属、比率、有限サイズを検証し、不正なLayoutは既定配置へ戻す。利用できないTabは除外する。
- Zoneの開閉とPanel個別の表示状態は別に保存する。最後のTabが移動したSide Panelは閉鎖中の状態を経て除去する。空のSide Zoneも再び開くことができる。
- Viewport幅の境界は `LayoutProfile` が決める。Wide は 960px 以上、Compact は 960px 未満とし、境界付近の往復を抑えるため Compact から Wide へは 960px、Wide から Compact へは 928px（960px − 32px のヒステリシス）で切り替える。
- `InteractionPolicy` は Profile と入力方式（Mouse / Touch）から操作の可否を決める。Compact では Panel の drag / resize / tab detach を無効化し、Context Menu は Mouse なら右クリック、Touch なら長押しとする。
- Compact では Panel 配置を永続化しない。Wide に戻したときは既定配置から始め、Wide で保存した配置だけを復元する。
- Viewport 幅・入力方式の参照は Shell と `kit` の共通 wrapper に限定し、Feature からは直接参照しない。`scripts/check_architecture.py` が Feature 配下の `viewport_size` / `user_agent` 参照を拒否する。

## UI の target 分割

- `clients/ui` は単一 crate（`desktop-ui`）のまま、target 別の責務を内側のモジュールで分ける。Dashboard の Layout は Shell が所有し、Desktop / Web は起動と window 生成だけを持つ。
- `kit` は tokens / theme / components / `LayoutProfile` / `InteractionPolicy` を持ち、Desktop と Web の両方から使う。kit は Host や Client の状態を所有しない。
- `desktop` はネイティブ window の起動（`run`）、window options、`window_host`（Window と Feature の所有）を持つ。Web ではビルドしない。
- `web` はブラウザーの起動（`run_web`）、wasm 用の backend 組み立て、prototype backend を持つ。ネイティブではビルドしない。
- 両 target が共有する `DesktopServices` / `InitialView` / `WindowFactory` / `open_desktop_window` は crate ルートの `app` に置く。target 固有のコードを `app` へ戻さない。
- 共通 UI 部品は `kit::components` の wrapper（popup / menu / scroll / row / surface など）を経由して使い、Feature が同じ見た目を個別に組み立てない。

### 描画系 Feature の wrapper 適用範囲

- 一覧・カード・検索結果の行は `kit::components::{hover_row, selectable_row}` を使う（ファイル、検索、Task、コマンド検索、設定のテーマカード）。
- Dialog は Feature と `root.rs` から `WorkspaceDialog` を直接呼ばず、`DocumentClient::choose_workspace_path` を経由する。`scripts/check_architecture.py` が `file_dialog()` の直接呼び出しを拒否する。
- Terminal と Editor も、外側の Scroll は `kit` の `scroll_viewport` を使う。ペインが空のときの表示は `shell/main/content.rs` が `panel_empty_state` で出し、Feature 側では持たない。
- Terminal のセル描画と Editor のテキスト描画は、GPUI の `Element` を直接実装して行う。理由は次のとおりで、`row` / `menu` 系の wrapper へは寄せない。
  - Terminal は可視行だけを仮想化して描画し、行ごとの色・属性・カーソルを `TerminalCell` から直接組む。行の高さと spacer が文字の実測に依存するため、wrapper を挟むと再計測が増える。
  - Editor は `ShapedLine` と IME / 選択範囲のハンドリングを `Element::paint` で行う。1文字単位の配置は wrapper ではなくテキスト整形の結果で決まる。
- したがって「Terminal / Editor の描画を共通 wrapper に寄せる」は対象外とする。共通化するのは外側の Scroll・空状態・操作ボタンまでとし、セル / 行の描画ロジックは各 Feature が持つ。

## Desktop の共通Tab操作

- `UiContribution` がViewの表示名・アイコン・カテゴリ・初期配置・複数起動方針・配置能力・表示条件を宣言し、`FeatureRegistry`が共通Pickerへ提供する。FeatureごとのTabBarやPanel所有者を作らない。
- 通知履歴はGPUI App全体で共有し、Rootごとに分かれるWindowGroupには所有させない。通知種別の既定PolicyがPopupへの配送を決め、発生時のActive Windowだけに最新3件まで一時表示する。Popupの終了と履歴の削除は別操作とする。CenterはPickerへ表示しない通常のTabとして共通Layoutを使う。
- Tabの追加・選択・非表示・Closeは配置状態を更新する操作であり、HostのDocument・Terminal・Agentを終了する操作ではない。
- ピン留めはPanelの共通Tab状態として保存し、左側に配置する。タブの単独・一括Closeではピン留め済みを対象から外すが、Windowやアプリの終了は妨げない。移動時はPINを維持する。
- ボタン起点のPopupは`components::Popup`でボタン直下へ、サブメニューは起点行の横へ配置する。クリック座標やShellに保存した位置は使わない。`PopupScope`は親子の表示範囲と起点ボタンを一つの操作範囲として外側クリックを判定する。Window端補正は共通部品、開閉・選択状態とEscape・Focus復帰は呼び出し側が所有する。`components::Menu`は`MenuEntry`の配列から行・アイコン・区切りを描画し、表示フォントで計測したラベル・ショートカット・矢印の最大行幅にpadding・borderを加えて幅を決める。呼び出し側に固定幅や文字数による幅推定を置かず、Window幅を上限にする。
- HeaderのZone操作ボタン群を非表示にしても、ViewメニューまたはCommand Searchから同じZone Commandを実行できる。

## Desktop のPanel配置・ドラッグ

- 通常のPanel面へのTab Dropは既存Panelへ移動する。Panel追加候補は成長方向の末端に限る。Leftは右、Rightは左、Bottomは上、Mainは追加不可。Sideは最大3 Panelで、上限時には追加候補を表示しない。
- 境界候補はポインタからの最短距離で選び、同距離はPanel IDと端の順序で安定させる。Previewと確定は同じ候補状態を検証し、確定後の配分に基づく矩形を表示する。
- Panel間の通常DragはResize。明示Resizeで閾値まで縮めたPanelだけを閉じる。Windowの縮小・復元・DPI変更・保存復元ではPanelや比率を変更しない。利用領域が不足した場合は保存比率を維持して縮小し、新規追加を拒否する。
- 同じWindowGroupのDocumentはWindowHostのDock/Undockを使用し、ToolのWindow間移動は受理しない。Window生成失敗や不正Dropでは元のTabとResourceを保持する。

## Desktop の Task / Workbench Mode

- TaskはProject配下の作業単位であり、Agent Sessionの所有物ではない。Task作成とAgent Session・Terminal開始は別のCommand/APIとし、Agent Sessionが0件でもTaskを成立させる。
- `WorkbenchMode::Ide`は選択Taskと作業環境のTaskを分離する。Task選択では`ConversationSession::switch`を呼ばず、Editor・Terminal・Git・Panelを維持する。Agent表示が選択Taskへ追従する場合、そのTaskのWorkspace IDをClient worker上で解決し、世代検証後に適用する。Terminal/Gitの対象は選択Taskから推測せず、維持している作業環境のIDを使う。`WorkbenchMode::Agent`ではTaskごとの`ConversationViewState`と作業環境を切り替える。Task切替でHost Resourceを閉じたり複製したりしない。
- Task選択・Mode変更・View所属変更は共通の`UiCommand`経路で処理する。View所属は表示上のScopeであり、`ViewInstanceId`、Document、Terminal、SessionのResource所有権を変更する契機にはしない。
- `ConversationViewState.workbench`にはMode・選択Task・View所属と、IDE用Layout・IDEの作業Task・AgentのTask別Layoutを保存する。Layoutの退避値にWorkbench自身を入れ子で複製しない。これらを永続化の変更判定にも含める。初回はIDEでも保存Layoutを復元し、以後のIDEのTask選択では復元しない。Mode切替は現在の表示を対応する保存先へ退避し、作業Taskの切替成功を確認してから確定する。旧保存値はIDE・空のBindingへ移行し、Host Resourceや実行中Sessionを永続化値から再生成しない。
- Task所属のViewはAgentで別Taskを選択すると非表示にし、IDEでは現在の配置を維持する。共有Toolの配置はTask Layoutの復元時にも個体IDを維持する。Terminalの表示コンテキストはViewのTask/Resource Bindingから解決し、共有Terminalは現在の接続先を維持する。Terminal参照はEditorや選択中Taskへ移さず、実際の接続先Taskへ保存する。再Attach・操作待ち・接続失敗の未確定表示を空のTerminal一覧として保存しない。Resource固定参照は対応済みFeatureと実在するIDに限定し、対象がなければ別Resourceへフォールバックしない。
- Task一覧は選択中Taskに属するAgent表示だけを描画する。別TaskのSessionを自動起動・自動追加せず、既存のTask / Worktree / Terminal / Agent開始操作を明示的な入口として再利用する。
- ACPプロトコルやAgent実行エンジンの新規実装は対象外とする。未対応機能を表示だけのButtonで完了扱いにせず、既存能力の範囲を共通RegistryとCommandで表現する。

## Desktop の Command / Keymap

- 名前付き操作は Command ID で識別し、画面操作と GPUI Action から既存処理へ接続する。Editor の文字入力と Quick Search の内部操作は別の入力 Binding とする。
- Keymap は Desktop が所有し、Command ごとの Binding 一覧を既定値、選択 Preset、User Override の順に解決する。各層の未指定は継承、明示解除は空、指定された一覧は下位層の全 Binding を置換する。先頭を表示用とする。未登録 Preset は既定値へ戻す。Preset と解決結果を User Settings へコピーしない。
- GPUI の Binding は解決結果の adapter であり、設定変更時は内部入力を含めて再登録する。他の登録元による変更を検知した場合は全消去せず再適用を拒否する。Host へ Keymap を同期しない。

## CLI の対象

- `clients/cli/` は利用者向けの非対話 CLI とし、models / protocol にのみ依存する。Host の管理操作や開発用の内部コマンドを持たない。
- Host の管理操作（device の承認・失効、credential の発行・失効、Workspace の登録）は `lapis-host` の標準入力で行う。
- 開発用のコマンド（Host の起動、Web のビルド、アーキテクチャ検査、smoke）は `scripts/` と `clients/desktop/app` に置き、利用者向け CLI には含めない。

## 依存の検査

`python scripts/check_architecture.py` は `cargo metadata --locked --no-deps` の全 workspace member と全 dependency（dev/build/target 条件を含む）を検査する。
許可行列は `scripts/check_architecture.py` が実行上の正本。models は models のみ、protocol は models、application は models、runtime は application/models/protocol、infrastructure は application/runtime/models、transport は runtime/protocol/models へ依存できる。各層内の依存も許可する。
Desktop の Composition Root は `clients/desktop/app/` として具体実装を組み立てる。通常の Client (`clients/cli/` 等) は models/protocol への依存に限定し、Backend の実装に直接依存させない。Desktop UI は `clients/state/` と models/protocol のみへ依存できる。`clients/state/` は models/protocol、Desktop 表示状態を保存する persistence は state/models のみへ依存できる。Backend からこれらの Client 実装への依存は dev-dependency も禁止する。
Backend と共通契約から GPUI/native dialog への依存は禁止。

既存違反は `scripts/architecture-exceptions.json` の完全一致した crate 対と理由に限定する。未使用例外も失敗するため、削除した違反を再導入する余地を残さない。新規違反を「移行中」として追加しない。

# 責務と依存方向

機能を追加・移行する開発者が、状態の所有者と配置先を判断するための正本。

| 領域 | 所有するもの | 所有しないもの |
| --- | --- | --- |
| `apps/` | Client の Window、Panel、Tab、focus、selection、表示 cache | Host 上の実行資源 |
| `shared/models/` | Transport 非依存の値・ドメイン規則・port | GPUI、通信、外部プロセスの具体実装 |
| `shared/protocol/` | request/response/event、revision、認可可能な操作契約 | OS 実装、Client の画面構成 |
| `backend/application/` | 操作と判断、port による外部連携 | GPUI、OS 固有 API |
| `backend/runtime/` | 正規状態、操作の直列化、購読・revision・実行資源の寿命 | Window、Panel、focus |
| `backend/infrastructure/` | FS、Git CLI、PTY、LSP、runner、永続化 | 画面の判断 |
| `backend/transport/` | 認証・wire・接続と共通 Runtime への配送 | 独立した Workspace/Document 正本 |
| `backend/host/` | Runtime と具体実装の組み立て、開始・終了 | UI |

## 配置の判断

状態とリソースの所有原則は [全体アーキテクチャ](../ARCHITECTURE.md) を参照する。追加実装の具体的な配置は次のとおり。

1. 値・規則・port は models、通信に公開する契約は protocol、操作判断は application。
2. 外部環境に触れる具体実装は infrastructure。認証や wire は transport。
3. 実際の所有・寿命の境界がなければモジュールで分割する。空 crate や転送だけの層を作らない。

## Runtime と接続の契約

- Host が登録した Workspace ごとに Runtime が Document の内容・revision・undo/redo、Terminal の出力・状態、Git/Task の操作範囲を所有する。Client の Document は表示 cache であり、編集は revision を指定して正本へ送る。
- 埋め込み接続と Remote は同じ request/response/event と Runtime を使う。Transport は認可と配送を担い、別の正規状態を作らない。
- Git/Task の外部 I/O は正規状態 worker の外で実行する。Runtime 内で接続・Workspace を検証し、Workspace ごとの実行資源を直列化したうえで、結果とイベントを worker に戻す。他 Workspace の操作や Document・Terminal の更新を Git の完了待ちにしない。
- Git/Worktree/Task の公開操作は Workspace・Worktree・Execution の ID を指定する。実行ディレクトリ、内部 TaskRecord、OS の絶対パスを wire に公開せず、実行 ID と初期状態は Runtime が生成する。
- Document の close や接続切断は attachment の解除であり、未保存内容・履歴・Terminal・Agent の終了ではない。実行終了には明示的な終了操作が必要。
- Document の古い revision は拒否する。保存済みの Client 表示状態を復元しても、すでに存在する正規 Document の新しい内容を上書きしない。
- 重なる Workspace grant から同じ canonical path を別の Document として開く操作は競合として拒否する。別 Workspace の内容を暗黙に共有して認可範囲を広げない。
- Terminal の購読欠落は snapshot と sequence で回復する。一時的な poll・通信失敗を実行終了とみなさず、出力の置換 snapshot と追記イベントを区別する。

## Desktop の更新境界

- Root は Entity の組み立てと機能間イベントの接続を担当する。Shell は Window・Panel・Tab・Layout を所有し、各 Feature の Backend 操作や表示 cache を集約しない。
- Feature は自身の入力・表示 cache・更新周期を所有する。共通 Client Adapter の worker で操作を直列化し、描画・入力処理から FS・Git・PTY や同期 port を直接呼び出さない。
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

Document は取り外しボタンと Root への再ドックに加え、Windows ではタブを Window 外で離すと新 Window を作り、同じグループの Panel へ離すと再ドックする。Root / 子 / 子同士を同じ移動経路で扱い、移動先で文書を選択する。空になった子 Window は開いたままとし、閉じる操作で解放する。配置モデルは Document / Tool 共通だが、Tool の Window 間移動は未有効。Terminal は focus と入力先・出力購読、Task / Agent は選択 execution とストリームの寿命を持つため、既存 Entity を再利用して再購読範囲を検証するまで移動させない。個別の Window Manager や汎用 Trait は増やさない。

GPUI 0.2.2 の Windows backend は MouseDown で `SetCapture`、MouseUp で `ReleaseCapture` し、GPUI は受信した Window の MouseUp 後に active drag を破棄する。Shell は元 Window の capture phase で解放を受け、`ClientToScreen` / `ScreenToClient` で各 Window の DPI をまたぐ物理座標を変換する。`WindowFromPoint` の手前の Window と描画時の Panel bounds を照合し、背後の Window へ移動しない。別グループの Lapis Window と、Lapis の Panel 以外の領域では取り消す。外部アプリ上では新 Window を生成する。新 Window は離した位置のモニターの作業領域内へ置く。GPUI の変更やイベントの定期監視は加えない。

ドラッグ開始時には配置を変更しない。Escape は GPUI の key binding より前に処理し、focus を失った場合も取り消す。取消後の MouseUp、古い所属、終了中・入力処理中、Tool の Window 間移動は受理しない。同一Window内の移動先はOverlayで強調する。Window外の追従プレビューと別Windowの移動先の強調表示は未実装。Windows 以外では既存の Window 内ドラッグとボタン操作を維持するが、Window 間ドラッグは未対応。

WindowGroup の再起動復元は未実装。後続では group / window の永続 ID、親子関係、bounds、Panel ごとのタブ順・Active Tab・サイズを、Document の復元用 ID と関連づけて保存する。現状の HostId / GPUI EntityId を永続 ID に使わない。明示 close 後の更新済み Document 一覧を保存し、閉じたタブ・Window を復活させない。既存の文書復元は OS Window の配置復元を意味しない。

`scripts/test_window_host.ps1` は隔離した Workspace と設定で Native Window の生成・共有編集・再ドック・グループ分離・終了・再生成と CPU の短時間サンプルを検証する。Windows では自プロセスのテスト Window だけに Mouse / Key メッセージを送り、Escape、Window 外への取り外し、重なった Window の再ドック、子同士の移動、別グループへの拒否、文書領域の高さも確認する。対象 Window の表示と重なり順はこの明示的な smoke モードだけで操作する。保存確認ダイアログ・IME・複数 DPI のモニターをまたぐ実操作と継続性能は別途確認する。

## Desktop の可変 Layout

- `DockLayout`はLeft・Main・Right・BottomのZoneとZoneの配置・表示を所有する。各ZoneはPanelの順序、比率、開閉状態を持ち、PanelはTabの順序と選択を持つ。Panelの所属ZoneはZone側で管理し、Panelには重複保持しない。PanelやZoneはDocument・Terminal・Agent等のResourceを所有しない。
- `ZonePolicy`はZoneの開閉・Panel数・成長方向・サイズ制約を定義する。Mainは常に開き、削除不能な一つのPanelを持つ。Zone内部の順序と比率は`PanelLayout`で表し、初期状態もPanel追加後も同じRendererを使う。空ZoneはTool Pickerから初回Panelを作成する。
- `DockGeometry`はViewportとLayoutからFrame・Zone・Panel・境界を計算し、描画と操作に使用する。RootとDetachedは同じFrameを使う。Zone操作は共通Commandを経由する。
- `ViewId`は種類、`ViewInstanceId`は個体を識別する。Layoutの保存値はZone・Panel・Tab参照だけを含む。旧Layout形式は復元せず、Task・未保存Documentなど別の保存値は保持する。
- 復元時はZone・Panel IDとTab個体の一意性、所属、比率、有限サイズを検証し、不正なLayoutは既定配置へ戻す。利用できないTabは除外する。
- Zoneの開閉とPanel個別の表示状態は別に保存する。最後のTabが移動したSide Panelは閉鎖中の状態を経て除去する。空のSide Zoneも再び開くことができる。

## Desktop の共通Tab操作

- `UiContribution` がViewの表示名・アイコン・カテゴリ・初期配置・複数起動方針・配置能力・表示条件を宣言し、`FeatureRegistry`が共通Pickerへ提供する。FeatureごとのTabBarやPanel所有者を作らない。
- Tabの追加・選択・非表示・Closeは配置状態を更新する操作であり、HostのDocument・Terminal・Agentを終了する操作ではない。
- Popupはクリック元の座標をアンカーにし、共通Popup基盤でWindow端補正、外側クリック、Escape、キーボード選択、Focus復帰を扱う。固定画面座標やFeature固有の補正値を使わない。
- HeaderのZone操作ボタン群を非表示にしても、ViewメニューまたはCommand Searchから同じZone Commandを実行できる。

## Desktop のPanel配置・ドラッグ

- 通常のPanel面へのTab Dropは既存Panelへ移動する。Panel追加候補は成長方向の末端に限る。Leftは右、Rightは左、Bottomは上、Mainは追加不可。Sideは最大3 Panelで、上限時には追加候補を表示しない。
- 境界候補はポインタからの最短距離で選び、同距離はPanel IDと端の順序で安定させる。Previewと確定は同じ候補状態を検証し、確定後の配分に基づく矩形を表示する。
- Panel間の通常DragはResize。明示Resizeで閾値まで縮めたPanelだけを閉じる。Windowの縮小・復元・DPI変更・保存復元ではPanelや比率を変更しない。利用領域が不足した場合は保存比率を維持して縮小し、新規追加を拒否する。
- 同じWindowGroupのDocumentはWindowHostのDock/Undockを使用し、ToolのWindow間移動は受理しない。Window生成失敗や不正Dropでは元のTabとResourceを保持する。

## 依存の検査

`python scripts/check_architecture.py` は `cargo metadata --locked --no-deps` の全 workspace member と全 dependency（dev/build/target 条件を含む）を検査する。
許可行列は `scripts/check_architecture.py` が実行上の正本。models は models のみ、protocol は models、application は models、runtime は application/models/protocol、infrastructure は application/runtime/models、transport は runtime/protocol/models へ依存できる。各層内の依存も許可する。
Host と Desktop の `apps/desktop/app/` は Composition Root として具体実装を組み立てる。通常の Client (`apps/cli/` 等) は models/protocol への依存に限定し、Backend の実装に直接依存させない。Desktop UI は `apps/desktop/client/` と models/protocol のみへ依存できる。client は models/protocol、Desktop 表示状態を保存する persistence は client/models のみへ依存できる。Backend からこれらの Client 実装への依存は dev-dependency も禁止する。
Backend と共通契約から GPUI/native dialog への依存は禁止。

既存違反は `scripts/architecture-exceptions.json` の完全一致した crate 対と理由に限定する。未使用例外も失敗するため、削除した違反を再導入する余地を残さない。新規違反を「移行中」として追加しない。

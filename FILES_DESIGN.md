# Files 機能設計

## 目的

Workspace のファイルを、規模に依存せず即応的に閲覧・選択できる単一ツリーとして提供します。ファイル列挙、外部変更監視、表示状態、言語判定、アイコン、将来のシンタックスハイライトを、それぞれの変更理由に沿って分離しながら、利用側からは一つの Files 機能として扱える境界を定めます。

この文書は実装対象と判断を記録します。長期的に維持するシステム境界は `ARCHITECTURE.md` を正とします。

## 対象

- Workspace ルートを起点とする単一ファイルツリー。
- ディレクトリ単位の遅延列挙と仮想化表示。
- Workspace に結び付くファイルシステム監視と差分再取得。
- ファイル名からの言語判定とファイルアイコン解決。
- 将来のシンタックスハイライトを追加するための契約。

次は対象外です。

- ファイルやディレクトリの作成、削除、移動、名前変更を行う UI。
- `.gitignore` や利用者設定に基づく除外規則。
- symlink の追跡と symlink 経由のファイル編集。
- Tree-sitter など特定の構文解析器の導入。
- OPEN DOCUMENTS セクション。開いている文書はエディタのタブが表現します。

## 所有と依存

```text
Desktop FilesFeature ────────> Workspace Files contracts <──────── Local adapter
        │                                  │
        ├──> LanguageRegistry              └──> Workspace identity
        │          │
        │          └──> optional syntax highlighter
        └──> File icon theme

Editor ──────────────────────> Document / Revision
```

### Workspace Files feature

`features/workspace-files` は Workspace 内のファイルを列挙し、変更を監視する能力を所有します。監視 resource は Workspace の開始と終了に結び付く独立したライフサイクルを持つため、core の `Workspace` モデルや `Document` I/O に含めません。

この feature は次を所有します。

- ディレクトリ列挙の契約と結果型。
- 監視の開始、event 取得、停止の契約。
- 外部実装のeventをLapisの意味へ変換したevent型。
- 一覧取得と監視に共通するエラー分類。

OSのパス解決、`std::fs`、監視ライブラリの型、threadやchannelの具体実装は所有しません。

### Local adapter

`backend/local` の Workspace Files adapter は、local Workspace のルートと相対パスを実パスへ解決し、ディレクトリ列挙と監視を実装します。外部エラーと監視eventはfeatureの契約へ変換し、`notify`固有の型を境界外へ出しません。

### Application boundary

application 層の `WorkspaceFilesSession` は、現在開いている Workspace、監視handle、Workspace世代、ディレクトリごとのrequest sequenceを所有します。Workspaceを切り替えると旧監視を停止し、世代を進め、未完了requestを論理的に無効化します。

このsessionは正規のディレクトリツリーを重複所有しません。backendから得たsnapshotをDesktopへ渡し、監視eventを再取得要求へ変換します。

### Desktop FilesFeature

`apps/desktop/ui/src/features/files` の `FilesFeature` は次のclient状態を所有します。

- 展開中のディレクトリ。
- 選択中のentry。
- ディレクトリごとの `Unloaded`、`Loading`、`Loaded`、`Error` 状態。
- backend snapshotから得た置換可能な子entry cache。
- request sequenceとWorkspace世代。
- ファイルツリーのscroll状態。

Document内容、保存状態、LSP状態、LanguageRegistry、監視resourceは所有しません。Filesのstate、action、viewはFiles配下へ置き、EditorやShellへ個別の状態を追加しません。独立した非同期更新と再描画範囲が必要になるため、実装時はFiles専用のGPUI `Entity`とします。

### Language と表示

`features/language` は `LanguageId` とパス判定規則を所有します。app entry pointで一つの `LanguageRegistry` を組み立て、Files、Editor、Problemsへ共有します。ProblemsやLSPを無効にしても言語判定は利用できます。

アイコンのassetとthemeはDesktopが所有します。Language側はSVGやGPUIの型を参照せず、Desktopのfile icon themeが `LanguageId` をassetへ解決します。対応するassetがない言語には汎用ファイルアイコンを使います。

## Workspace Files 契約

以下は契約の意味を示す型です。実装時も外部ライブラリ固有型を追加しません。

```rust
pub struct WorkspacePath(PathBuf);

pub enum FileEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

pub struct DirectoryEntry {
    pub path: WorkspacePath,
    pub display_name: String,
    pub kind: FileEntryKind,
}

pub struct DirectorySnapshot {
    pub workspace_id: WorkspaceId,
    pub directory: WorkspacePath,
    pub entries: Vec<DirectoryEntry>,
}

pub struct WorkspaceWatchId(/* opaque */);

pub enum WorkspaceFileEvent {
    DirectoryInvalidated {
        watch_id: WorkspaceWatchId,
        directory: WorkspacePath,
    },
    RescanRequired {
        watch_id: WorkspaceWatchId,
    },
    WatchFailed {
        watch_id: WorkspaceWatchId,
        message: String,
    },
}

pub trait WorkspaceFilesBackend: Send + Sync {
    fn list_directory(
        &self,
        workspace: &Workspace,
        directory: &WorkspacePath,
    ) -> Result<DirectorySnapshot, WorkspaceFilesError>;

    fn start_watch(
        &self,
        workspace: &Workspace,
    ) -> Result<WorkspaceWatchId, WorkspaceFilesError>;

    fn poll_events(
        &self,
        watch_id: &WorkspaceWatchId,
    ) -> Result<Vec<WorkspaceFileEvent>, WorkspaceFilesError>;

    fn stop_watch(
        &self,
        watch_id: &WorkspaceWatchId,
    ) -> Result<(), WorkspaceFilesError>;
}
```

`WorkspacePath` はWorkspaceルートからの相対パスだけを表します。絶対パス、prefix、root component、`..`を許可せず、生成時に検証します。空のパスはWorkspaceルートを表します。UIは文字列を連結して `WorkspacePath` を作らず、backendが返した値をqueryやcommandへ渡します。非UTF-8名は内部のpath identityで保持し、`display_name`だけをlossy表示してidentityを失わないようにします。

`DirectorySnapshot` は一回のqueryで確定した子entryの全置換snapshotです。部分更新を正規状態にせず、監視eventを受けた場合も対象ディレクトリを再queryします。

一覧はディレクトリを先にし、同じkindでは表示名のASCII大小文字を無視して昇順、元の名前をtie-breakerとして安定化します。除外対象は現行どおり、どの階層でも名前が `.git`、`target`、`.DS_Store` と一致するentryです。

symlinkは `Symlink` として表示しますが、列挙もopen commandも行いません。これによりWorkspace外への脱出と循環を防ぎます。socketやdeviceなど通常ファイルでないentryは `Other` として表示するだけにします。

## Query と監視の流れ

Workspaceを開くときは次の順序を守ります。

1. 旧Workspaceの監視を停止する。
2. Workspace世代を進め、Filesの展開、選択、cacheを初期化する。
3. 新Workspaceの再帰監視を開始する。
4. Workspaceルート直下のqueryを開始する。
5. root snapshotを適用した後、監視開始以降に届いたeventを処理する。

監視を先に開始することで、初期query中の作成や削除を取りこぼしません。監視開始に失敗してもroot queryは続行し、Filesパネルに非致命的な警告と手動更新を表示します。

queryはUI threadで実行しません。各requestはWorkspace世代とディレクトリ単位の単調増加sequenceを持ちます。応答時にどちらかが現在値と一致しなければ破棄します。同じディレクトリへの新しいrequestを開始しても、古いsnapshotでcacheを上書きしません。

### Local watcher

local adapterは実装時にworkspace dependencyとして安定版 `notify 8.2` を追加し、`recommended_watcher` と再帰監視を使います。watcherはadapterが保持し、dropまたは `stop_watch` で確実に停止します。

raw eventは100msの窓でまとめ、影響する親ディレクトリを重複排除します。

- createとremoveは対象の親を無効化する。
- renameとmoveは旧pathと新pathの両方の親を無効化する。
- ディレクトリ自体のrenameまたはremoveは、その親と読み込み済み子孫を無効化する。
- ファイル内容だけのmodifyはツリーを無効化しない。開いているDocumentの外部変更検出はDocument側が所有する。
- eventのpathをWorkspace相対pathへ安全に変換できない場合、event欠落やoverflowが疑われる場合は `RescanRequired` にする。
- watcher自体の継続不能なエラーは `WatchFailed` にする。

`DirectoryInvalidated` を受けたとき、展開中のディレクトリは直ちに再queryします。折りたたまれているディレクトリは `Unloaded` に戻し、次回展開時にqueryします。`RescanRequired` はrootと、現在展開中の全ディレクトリを浅い階層から再queryします。

Desktopは100ms間隔でsessionのeventをpollし、変化があるときだけFiles Entityを再描画します。Workspace切替、切断、window終了では監視を停止します。再接続時は監視を作り直して `RescanRequired` と同じ再取得を行います。

## ツリーUI

Filesパネルは、Workspaceルートとその子孫だけを表示する単一ツリーです。ルート行にはWorkspace名と補助情報としてroot pathを表示し、初期状態で展開します。子ディレクトリは折りたたんだ状態から開始します。

表示対象は、読み込み済みsnapshotと展開集合から毎回平坦な可視行へ変換します。固定の500件上限は設けず、GPUIの仮想リストで可視範囲だけを描画します。

- Directoryのchevron clickと行のdouble clickは展開状態を切り替える。
- Fileのsingle clickは選択を変更し、Document open commandを送る。Enterも選択中のFileを開く。
- Directoryのsingle clickは選択だけを変更する。
- `Loading` は対象Directory直下に一行のloading表示を置く。
- `Error` は対象Directory直下にエラーとretry actionを置き、他の枝は維持する。
- 更新actionはrootと現在展開中のディレクトリを再queryする。
- Workspace切替時はscrollを先頭へ戻す。通常のsnapshot更新では選択、展開、scrollを維持する。

## 言語判定とファイルアイコン

`LanguageDefinition` は次のmatcherを持ちます。

```rust
pub struct LanguageDefinition {
    pub id: LanguageId,
    pub exact_file_names: Vec<String>,
    pub suffixes: Vec<String>,
    pub extensions: Vec<String>,
}
```

判定優先度は完全ファイル名、最長suffix、拡張子です。同じ種類では登録順に依存せず、重複matcherをregistry構築時のエラーにします。既存挙動を維持するためmatcherはASCII大小文字を区別しません。不明なpathは `None` とし、Filesは汎用アイコン、Editorは通常のテキスト表示を使います。

file icon themeは `LanguageId` からfile icon asset IDへのmapを持ちます。言語を追加するだけならLanguageDefinitionの登録だけで成立し、専用assetは任意です。assetを追加する場合も、Filesのviewに言語別のmatchを追加しません。

## 将来のシンタックスハイライト

LanguageRegistryは、将来 `LanguageId` ごとのhighlighter providerを任意登録できる境界を持ちます。具体的なparser、grammar、query、incremental parse stateはprovider adapter内に閉じます。

```rust
pub struct HighlightSpan {
    pub byte_range: Range<usize>,
    pub style: SyntaxStyleId,
}

pub struct HighlightSnapshot {
    pub document_id: DocumentId,
    pub revision: Revision,
    pub language_id: LanguageId,
    pub spans: Vec<HighlightSpan>,
}
```

`SyntaxStyleId` は `keyword`、`string`、`type` などの意味を表し、色やfontはDesktop themeが解決します。GPUI描画は可視行と交差するspanから複数の `TextRun` を作ります。Documentの現在RevisionとsnapshotのRevisionが一致しない場合は適用しません。

構文解析器の採用は、Rust一言語で編集遅延、初期解析時間、増分更新、Windows buildを計測してから決めます。この時点では公開契約を解析器固有の都合で変更しません。

## エラーと回復

- 一つのDirectoryの権限エラーでWorkspace全体を開けなくしない。Directory単位の `Error` として保持する。
- entry単位のmetadata取得失敗は、そのentryを表示できない理由をDirectory queryのエラーへ含め、黙って欠落させない。
- watcherが利用できない環境では遅延queryと手動更新を継続する。
- backend切断中のqueryは失敗として表示するが、最後に確定したsnapshotを消さない。
- 再接続時はwatcherを再作成し、展開中のDirectoryを再取得する。
- root自体が削除された場合はroot snapshotを無効化し、Workspace unavailable状態を表示する。

## 移行順序

1. Workspace Filesの契約、local列挙adapter、application sessionを追加する。
2. `EditorSession`から `file_tree` と再帰 `list_tree` を外し、Document I/O契約と分離する。
3. Files専用Entityへstate、action、viewを移し、rootの遅延queryと仮想リストを実装する。
4. local watcherと意味的event、再取得、手動更新を接続する。
5. LanguageRegistryをapp entry pointの共有依存へ移し、matcherとicon themeをdata-drivenにする。
6. Rust一言語のhighlighter spikeを別変更として行い、Revision付きsnapshotを検証する。

各段階で動作する経路だけを公開し、後続段階の空moduleや利用不能なcapabilityは作りません。

## 検証と受入条件

### 契約とadapter

- root queryが直下entryだけを返し、子孫を再帰列挙しない。
- Directoryが先、同じkindは安定した名前順になる。
- `.git`、`target`、`.DS_Store`を除外し、その他のdot directoryを表示する。
- 空Directory、権限エラー、非UTF-8名、symlink loopを独立して検証する。
- 500件を超えるDirectoryも欠落させない。

### 監視

- 外部のcreate、remove、rename、別Directoryへのmoveで影響する親だけを無効化する。
- event stormを100msの窓で統合する。
- overflow相当の入力を `RescanRequired` へ変換する。
- Workspace切替後の旧eventを適用せず、旧watcherを停止する。
- watcher開始失敗、途中失敗、backend再接続から手動または自動再取得で回復する。

### Desktop

- rootだけが初期展開され、子Directoryは展開するまでqueryされない。
- 遅いqueryを逆順に完了させても新しいsnapshotを古いsnapshotで上書きしない。
- 展開、選択、scrollをsnapshot更新後も維持する。
- loading、Directory単位error、retry、手動更新を表示する。
- 500件を超えるentryを仮想化して操作できる。
- Windowsの実GPUと対話desktopがある環境で対象画面を撮影し、単一ツリー、indent、chevron、選択、scrollを目視確認する。

### Language と将来のhighlight

- 現在対応するRust、Markdown、JavaScript/JSX、TypeScript/TSX、Go、Kotlin、Javaの判定を維持する。
- 完全ファイル名、複合suffix、拡張子の優先順位と重複登録エラーを検証する。
- 専用アイコンがない言語と不明なファイルが汎用アイコンへfallbackする。
- 将来のhighlighter testでは古いRevisionの `HighlightSnapshot` を描画へ適用しない。

# ROADMAP.md

Lapisは、GPUI Desktopを主軸に、KMP MobileのAndroidとiOSからも同じRust backendを利用できる軽量な開発Workspaceを目指します。Web clientは現在の対象に含めません。「Web開発」はWeb技術のプロジェクトを編集する意味です。

## Phase 0: 開発基盤

アーキテクチャ境界、共通UI、検証方針、Gitフックを整えます。完了済みです。

## Phase 1: 基本Web開発

対象画面のUIを完成させ、DesktopとMobileからWorkspaceへ接続し、HTML、CSS、JavaScript、TypeScript系ファイルの基本編集とTerminal実行を可能にします。remote動作の基盤を整え、既知の重大バグがない状態を完了条件とします。詳細は`PHASE_1.md`を正とします。

## Phase 2: コード理解と変更管理

シンタックスハイライトとGitを中心に、検索、LSP、Diagnosticsなど開発支援を追加します。詳細はPhase 1完了時に決めます。

各Phaseはmilestoneであり、実装は責務ごとの小さなfeature、fix、refactorブランチへ分けます。未着手Phaseの詳細仕様や空実装は作りません。

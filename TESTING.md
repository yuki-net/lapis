# TESTING.md

Lapisのテスト配置とコード変更の完了条件を定義します。

## 完了条件

変更領域と直接依存する領域のうち、必須プロファイルを実行します。CIでは製品・実行境界ごとにcheckを分け、どの領域が壊れたかを判別できるようにします。

## 検証状態

| 状態 | 扱い |
| --- | --- |
| 必須 | コード変更の完了条件として実行する |
| 未実装 | 対象のentry pointや検証基盤がまだ存在しない |
| 手動 | 実機または利用者が確認する |

## CIプロファイル

### Backend

Desktop UIを除くRust workspaceを検証します。対象は `backend/*` だけに限定せず、Backendから利用する `features/*` も含みます。

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --exclude lapis --exclude lapis-desktop-ui
cargo test --workspace --all-targets --exclude lapis --exclude lapis-desktop-ui
cargo clippy --workspace --all-targets --exclude lapis --exclude lapis-desktop-ui -- -D warnings
cargo build --workspace --all-targets --exclude lapis --exclude lapis-desktop-ui
```

### Desktop

Desktop固有のGPUI app/UIを対象OS上で検証します。

現在はWindowsのみを保証対象として実行し、workflowは将来Linux/macOSを追加できるmatrix構造にします。

```text
matrix:
- windows-2022
# - ubuntu-latest  # Desktop Linuxを保証する段階で有効化
# - macos-14       # Desktop macOSを保証する段階で有効化
```

各OSでは次を実行します。

```text
cargo check -p lapis -p lapis-desktop-ui --all-targets
cargo test -p lapis -p lapis-desktop-ui --all-targets
cargo clippy -p lapis -p lapis-desktop-ui --all-targets -- -D warnings
cargo build -p lapis --all-targets
```

UIの実画面・GPU描画・native window操作は引き続き手動確認です。

### CLI

#22で独立CLI entry pointを作成するまで未実装です。

CLIはDesktop GUIの全機能を再実装することを目的にしません。BackendをGUIなしで起動・設定・診断・管理するための薄いCUIとして設計し、必要になった操作だけ追加します。

独立後はCLI固有のbuild/test/smoke checkを追加します。最初から全OS対応を必須にはせず、必要に応じてmatrixを拡張します。

### Mobile

Linux runner上でKMPのAndroid/commonコードを検証します。

```text
cd apps/mobile
./gradlew :sharedUi:testDebugUnitTest
./gradlew :androidApp:assembleDebug
```

iOS native buildはこのprofileには含めません。必要になった段階でmacOS runnerを追加します。

### Web

現在のVite placeholderが壊れていないことだけを確認します。

```text
cd apps/web
npm ci
npm run build
```

#23でRust/WASMへ移行した後は、このprofileをWASM build/testへ置き換えます。

## GitHub Actions

PRと`develop` pushでは、次のcheckを独立して表示します。

| Check | 状態 |
| --- | --- |
| Backend | 必須 |
| Desktop (Windows) | 必須 |
| Mobile | 必須 |
| Web | 必須 |
| CLI | #22完了後に追加 |

将来のcross-client contract test、clean container build、Linux/macOS Desktop、CLIの追加OS、WASM検証は、それぞれの基盤ができた段階で追加します。

## テスト配置

| 対象 | 配置 |
| --- | --- |
| Rust内部 | 対象モジュールの`tests.rs` |
| Rust公開動作 | 対象crateの`tests/<behavior>.rs` |
| KMP | 各`*Test` source setの`<Subject>Test.kt` |
| GPUI公開動作 | `apps/desktop/ui/tests/` |
| OS・アプリ起動 | 対象appの`tests/` |

非公開ロジックのテストのために本番APIを広げません。fixtureとhelperは利用するテストの近くに置き、共有が必要になってから抽出します。

## テストレベル

低いレベルで再現できる振る舞いを、GPUIやOSテストだけで検証しません。

1. 状態遷移と純粋ロジックをunit testで検証する。
2. crate、module、adapterの契約をintegration testで検証する。
3. Entity、Action、focus、overlayをGPUI test contextで検証する。
4. 起動、native window、dialog、GPU描画を対象OSで確認する。

## WindowsのGPUI画面確認

- AIが確認できない場合は、未確認箇所と操作手順を利用者へ渡す。
- 対話中のユーザーセッションと実GPUを使用する。スリープ中は撮影できない。
- 無人撮影では実行中だけ`SetThreadExecutionState`または`PowerSetRequest`で消灯とスリープを防ぎ、完了後に解除する。
- GPU描画は`Windows.Graphics.Capture`またはDesktop Duplicationで取得する。`PrintWindow`とGDIだけを根拠にしない。
- 撮影できない環境では未検証として理由を報告し、利用者の画像を代替証跡にする。

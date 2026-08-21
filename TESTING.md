# TESTING.md

Lapisのテスト配置とコード変更の完了条件を定義します。

## 完了条件

変更領域と直接依存する領域のうち、必須プロファイルを実行します。未定義の領域は検証対象へ推測で加えず、影響の可能性を報告します。ドキュメントだけの変更は対象外です。

> 現在はpush運用ができないため、Gitフックでは必須プロファイルをpre-commitへ集約します。pre-pushは理由を添えてコメントアウトし、push再開時に戻します。

## 検証状態

| 状態 | 扱い |
| --- | --- |
| 必須 | コード変更の完了条件として実行する |
| 未定義 | 検証基盤を推測で作らず、影響の可能性を報告する |
| 手動 | 実機または利用者が確認する |

## 検証プロファイル

| 対象 | 状態 |
| --- | --- |
| Rust | 必須 |
| KMP | 未定義 |
| UI実画面 | 手動 |

Rustの必須検証は次のとおりです。

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
```

KMPは検証基盤ができた段階でコマンドを登録し、必須へ変更します。

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

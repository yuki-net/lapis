# Security Policy

## 脆弱性の報告

**公開の Issue / Discussion には書かないでください。**

GitHub の Private vulnerability reporting を使ってください。

- https://github.com/yuki-net/lapis/security/advisories/new

## 対象

- `develop` / `main` の最新
- 本プロジェクトは開発中のプロトタイプです。古い版へのバックポートはベストエフォートで対応します

## 対応の流れ

1. 報告を受け取ったら、7 日以内を目安に一次返信します（ベストエフォート）
2. 再現・影響範囲を確認し、非公開で修正を準備します
3. 修正のリリース後に、必要に応じて詳細を公開します
4. 希望があれば、報告者をクレジットします

## 対象外

- `docs/specs/` に記載済みの設計上の制約
- 依存 crate の既知の脆弱性（Dependabot / `cargo deny` で追跡します）

# 共通契約

- `models/` は値・ドメイン規則・port、`protocol/` は操作・応答・イベントの公開契約。
- Backend や Client に依存しない。GPUI、外部プロセス、接続、画面状態を持ち込まない。
- OS 固有の値を wire に漏らさず、入力制約とシリアライズの回帰テストを追加する。
- 依存方向の正本は `docs/specs/architecture.md`。将来のための空モジュールを作らない。

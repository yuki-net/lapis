# automation

Issue → 実装 → 役割別レビュー → マージを自動化するための設定・手順。

## 状態

- **M1（この PR）**: 仕様・policy・roles・認証と秘密・skill・秘密混入テスト
- M2 以降: 役割別レビュー（verifier + skeptic / architect / minimalist）、coordinator merge、dispatch、dashboard

dispatch / reviewer の実行コードはまだ無い。M2〜M4 で `automation/` と `reviewer/` に足す。

## 読む順

1. [docs/specs/automation.md](../docs/specs/automation.md)
2. [policy.toml](policy.toml)
3. [roles.toml](roles.toml)
4. [KEYS.md](KEYS.md)（owner 名義の認証と秘密）

## 予定しているコマンド

```powershell
# M3: Issue を worker に渡す（人間の明示トリガー）
python automation/dispatch.py --issue 123

# M2/M4: PR を役割別にレビューする（PR ごと・役割ごとに新しいセッション）
python -m reviewer review --pr 123
python -m reviewer serve            # オープン PR を poll
python -m reviewer dashboard        # :8787
```

## 秘密

- コミット・PR・merge は **owner 名義**。GitHub App / orcaworker / ブローカーは使わない
- AI キー / `.env` は repo に置かず、%USERPROFILE% 側に置く
- 詳細は [KEYS.md](KEYS.md)

## 注意

同一アカウント運用なので、「worker / reviewer は merge しない」「reviewer は書き込まない」は
GitHub の権限ではなく、**プロンプトと手順の制約**。保護パス / `.github/workflows` は人が対応する。

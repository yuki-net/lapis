# automation

Issue → 実装 → レビュー → マージを自動化するための設定・手順。

## 状態

- **M1（この PR）**: 仕様・policy・roles・App 手順・鍵の手順・skill・秘密混入テスト
- M2 以降: 鍵の境界（別 Windows ユーザー / サービスアカウント）、ブローカー、App 作成、dispatch、reviewer

dispatch / reviewer の実行コードはまだ無い。M2〜M4 で `automation/` と `reviewer/` に足す。

## 読む順

1. [docs/specs/automation.md](../docs/specs/automation.md)
2. [policy.toml](policy.toml)
3. [roles.toml](roles.toml)
4. [apps.md](apps.md)（GitHub App の作成）
5. [KEYS.md](KEYS.md)（鍵と token、境界の 2 案）

## 予定しているコマンド

```powershell
# M3: Issue を worker に渡す（人間の明示トリガー）
python automation/dispatch.py --issue 123

# M4: PR をレビューする（PR ごとに新しいセッション）
python -m reviewer review --pr 123
python -m reviewer serve            # オープン PR を poll
python -m reviewer dashboard        # :8787
```

## 秘密

- App private key / AI キー / `.env` はこのディレクトリにも repo にも置かない
- 置き場はブローカーのアカウント（`automation/KEYS.md`）
- オーナーの `gh` keyring は使わない
- 詳細は [KEYS.md](KEYS.md)

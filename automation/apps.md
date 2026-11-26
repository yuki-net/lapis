# GitHub App の作り方

worker / reviewer / 3 人目（judge など）は、すべて同じ手順で作る。
**鍵は repo に置かない。** ブローカーのアカウントだけが読める場所に置く。

## 1. App を作る

組織 `yuki-net` の Settings → Developer settings → GitHub Apps → **New GitHub App**。

| 項目 | 値 |
| --- | --- |
| Name | `lapis-worker` / `lapis-reviewer` / `lapis-<role>` |
| Homepage URL | 任意（repo の URL でよい） |
| Webhook | **Active を外す** |
| Where can this GitHub App be installed? | Only on this account |

## 2. 権限

### worker

| 権限 | 値 |
| --- | --- |
| Contents | **Read and write**（push / merge に必要） |
| Pull requests | **Read and write**（PR 作成） |
| Metadata | Read（必須） |

`Workflows: Read and write` は、worker が `.github/workflows/` を変更する必要が
出たときだけ付ける。`.github/` は保護パスなので、通常は付けない。

### reviewer

| 権限 | 値 |
| --- | --- |
| Contents | **Read** |
| Pull requests | **Read** |
| Metadata | Read（必須） |

reviewer は **GitHub に書き込まない**。コメント・ラベル・merge はしない。

## 3. 追加で必要になるかもしれない権限（未確定）

- **CI を App token で読む場合**: `Checks: Read` と `Commit statuses: Read` が必要。
  `gh pr view --json statusCheckRollup` / `gh pr checks` はこの 2 つを要求する。
  coordinator（オーナー token）が CI を読むなら、App には付けない。
- **ラベルを App で付ける場合**: `Issues: Read and write` が必要。
  coordinator が付けるなら、worker / reviewer には付けない。
- `Checks` / `Commit statuses` の **Write は付けない**（required check を偽造できる）。

## 4. インストール

1. App の **Install App** から `yuki-net/lapis` にインストール
2. App ID と Installation ID を控える
3. **Generate a private key** で `.pem` をダウンロード（一度しか出ない）

## 5. 鍵の置き場

`automation/KEYS.md` の境界（別 Windows ユーザー / サービスアカウント）に従う。

- 鍵はブローカーのアカウントだけが読める場所に置く（ACL）
- `roles.toml` には `key_ref` だけを書き、パスや鍵は書かない
- オーナーの `gh` keyring は使わない

## 6. reviewer の追加設定

- **Review / Protected の bypass には追加しない**（reviewer は merge しない）
- `reviews` team には**入れない**
- worker App も bypass には入れない

## 7. 3 人目以降

1. 同じ手順で App を作る（read-only を基本）
2. `automation/roles.toml` に `[roles.<name>]` を足す
3. ブローカーに `key_ref` の鍵を置く
4. 必要なら ruleset の bypass を追加する（Protected には追加しない）

コードの変更は不要。`roles.toml` とブローカーの鍵の配置だけで増やせる。

## 8. 確認

- `gh api -i /app` は App の JWT で確認できる
- installation token は `automation/KEYS.md` の手順で発行する
- worker token で `gh pr create` ができること
- reviewer token で `gh pr view` ができ、`gh pr comment` / `gh pr merge` は失敗することを確認する

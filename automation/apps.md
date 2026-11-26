# GitHub App の作り方

worker / reviewer / 3 人目（judge など）は、すべて同じ手順で作る。
**鍵は repo に置かない。** `orcaworker` ユーザー（または B を重ねる場合はサービスアカウント）
だけが読める場所に置く。

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
| Contents | **Read and write**（push に必要） |
| Pull requests | **Read and write**（PR 作成） |
| Checks | **Read**（CI を読む） |
| Commit statuses | **Read**（status を読む） |
| Metadata | Read（必須） |

- `Issues: Write` は付けない（ラベルは coordinator）
- `Workflows: Write` は常用しない。`.github/workflows` の変更は人が行い、必要な PR だけ一時的に昇格する

### reviewer

| 権限 | 値 |
| --- | --- |
| Contents | **Read** |
| Pull requests | **Read** |
| Checks | **Read** |
| Commit statuses | **Read** |
| Metadata | Read（必須） |

reviewer は **GitHub に書き込まない**。コメント・ラベル・review・merge はしない。

## 3. 共通の注意

- `Checks` / `Commit statuses` は **Read だけ**。Write を付けると required check を偽造できる
- `Issues: Write` は coordinator のみ。worker / reviewer には付けない
- `Checks: Read` / `Commit statuses: Read` が無いと `gh pr view --json statusCheckRollup` /
  `gh pr checks` が `Resource not accessible by integration` で失敗する

## 4. インストール

1. App の **Install App** から `yuki-net/lapis` にインストール
2. App ID と Installation ID を控える
3. **Generate a private key** で `.pem` をダウンロード（一度しか出ない）

## 5. 鍵の置き場

`automation/KEYS.md` の A（`orcaworker` ユーザー）に従う。

- 鍵は `C:\Users\orcaworker\.lapis\secrets\<role>\app.pem` に置く（ACL: `orcaworker`）
- `roles.toml` の `private_key` は **App 作成後に埋める**。それまでは空のまま
- オーナーの `gh` keyring は使わない

B（サービスアカウント + localhost ブローカー）を重ねる場合は、鍵を
`C:\ProgramData\lapis\secrets\` に移し、ACL を `lapis-broker` だけにする。

## 6. reviewer の追加設定

- **Review / Protected の bypass には追加しない**（reviewer は merge しない）
- `reviews` team には**入れない**
- worker App も bypass には入れない

## 7. 3 人目以降

1. 同じ手順で App を作る（read-only を基本）
2. `automation/roles.toml` に `[roles.<name>]` を足す
3. `orcaworker` の `secrets\<name>\app.pem` を置く
4. 必要なら ruleset の bypass を追加する（Protected には追加しない）

コードの変更は不要。`roles.toml` と鍵の配置だけで増やせる。

## 8. 確認

- `gh api -i /app` は App の JWT で確認できる
- installation token は `automation/KEYS.md` の手順で発行する
- worker token で `gh pr create` と `gh pr checks` ができること
- reviewer token で `gh pr view` / `gh pr checks` ができ、
  `gh pr comment` / `gh pr merge` は失敗することを確認する

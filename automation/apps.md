# GitHub App の作り方

worker / reviewer / 3 人目（judge など）は、すべて同じ手順で作る。
**鍵は repo に置かない。** VM の各ユーザーの home に置く。

## 1. App を作る

組織 `yuki-net` の Settings → Developer settings → GitHub Apps → **New GitHub App**。

| 項目 | 値 |
| --- | --- |
| Name | `lapis-worker` / `lapis-reviewer` / `lapis-<role>` |
| Homepage URL | 任意（repo の URL でよい） |
| Webhook | **Active を外す** |
| Where can this GitHub App be installed? | Only on this account |

## 2. 権限

Repository permissions:

| 権限 | 値 |
| --- | --- |
| Contents | **Read and write**（push / merge に必要） |
| Pull requests | **Read and write**（PR / review / merge） |
| Issues | **Read and write**（PR のラベル・コメント） |
| Checks | **Read**（CI の結果を読む） |
| Commit statuses | **Read**（status を読む） |
| Metadata | Read（必須） |

付与しないもの（write にしないもの）:

- Actions
- Workflows
- Administration
- Secrets
- Environments
- Checks の Write
- Commit statuses の Write

> `Checks` / `Commit statuses` は **Read だけ**にする。Write を付けると required check を
> 偽造できる。worker / reviewer のどちらにも Write を付けない。
> Read が無いと、reviewer は CI の結果を読めない。

## 3. インストール

1. App の **Install App** から `yuki-net/lapis` にインストール
2. App ID と Installation ID を控える
3. **Generate a private key** で `.pem` をダウンロード（一度しか出ない）

## 4. 鍵の置き場

VM の各ユーザーで:

```bash
mkdir -p ~/.config/lapis/worker
install -m 600 /path/to/lapis-worker.pem ~/.config/lapis/worker/app.pem
```

`~/.config/lapis/worker/env` に App ID / Installation ID を書く（600）。

`roles.toml` には **パスだけ**を書き、鍵は書かない。

## 5. reviewer の追加設定

reviewer App だけ:

1. ruleset `Review` の **bypass actors** に `Integration` として追加する
   - `bypass_mode: pull_request`
2. `Protected` の bypass には**追加しない**
3. `reviews` team には**入れない**（Code Owner 承認を満たさせない）

worker App はどちらの bypass にも入れない。

## 6. 3 人目以降

1. 同じ手順で App を作る
2. `automation/roles.toml` に `[roles.<name>]` を足す
3. VM のユーザーと `~/.config/lapis/<name>/app.pem` を用意する
4. 必要なら ruleset の bypass を追加する（Protected には追加しない）

コードの変更は不要。`roles.toml` と鍵の配置だけで増やせる。

## 7. 確認

- `gh api -i /app` は App の JWT で確認できる
- installation token は `automation/KEYS.md` の手順で発行する
- token で `gh pr list` が見えること、`gh pr merge` は worker では失敗することを確認する

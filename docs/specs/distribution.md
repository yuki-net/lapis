# 配布 package・manifest・install layout

この文書は、配布物の生成と配置、manifest の契約、install layout とデータ世代の固定事項を所有します。launcher 本体と更新 UX は別 Issue で、ここでは「何を配り、どこへ置き、何を検証するか」だけを決めます。

実装の正本は `scripts/package/distribution.py` と `scripts/test_distribution.py` です。schema の値や命名を変えるときは両方とこの文書を同時に更新します。

## 配布経路は1本

package の生成、manifest の署名、asset の SHA-256・size 検証は共通です。違うのは配置先と取得元だけで、`--dest` / `--root` の入り口で切り替えます。

| 配置先 | 指定 | asset の参照 |
| --- | --- | --- |
| ローカルフォルダ | `--dest local:<directory>` | manifest からの相対パス |
| 静的配置（自宅サーバー等） | `--dest local:<directory> --asset-base https://host/path` | 絶対 URL |
| GitHub Releases | `--dest github:<tag>` | `https://github.com/<repo>/releases/download/<tag>/<name>` |

取得側も同じ検証関数を使います。

| 取得元 | 指定 |
| --- | --- |
| フォルダ | `--root local:<directory>` |
| 静的配置 | `--root url:<base-url>` |
| GitHub Releases | `--root release:<tag>`（`gh release download` で取得） |

manifest は取得元を基準に相対パスを解決するため、同じフォルダをそのまま静的配置へ置けます。GitHub Releases では manifest と asset が同じ tag に入るので、相対パスでも解決します。

## manifest schema

初日から固定します。項目の追加・削除は旧 client が解釈できなくなるため、version を上げる以外の変更をしません。

```json
{
  "schema": 1,
  "version": "0.2.0",
  "breaking": false,
  "release_notes_url": "https://github.com/yuki-net/lapis/releases/tag/v0.2.0",
  "assets": {
    "windows-x86_64": {
      "asset": "lapis-0.2.0-windows-x86_64.zip",
      "sha256": "<64 hex>",
      "size": 12345678
    }
  },
  "signature": {
    "algorithm": "ed25519",
    "key_id": "lapis-release-1",
    "value": "<128 hex>"
  }
}
```

- `schema` は manifest 形式の版。`version` は配布するアプリの版。
- `breaking` は軸B（データ世代が上がる破壊的変更）かどうか。true の更新は launcher が新規 install クラスとして扱う。
- `release_notes_url` は http(s) の絶対 URL。
- `assets` はプラットフォーム名（`windows-x86_64` 等）ごとに asset の参照・SHA-256・byte 数を持つ。
- `signature.value` は `signature` を除いた payload を、key 順を昇順・区切りを `,`/`:` に固定した UTF-8 JSON に対して Ed25519 で署名した hex。

asset 名は `lapis-<version>-<platform>.zip` に固定します。zip の root は `versions/<version>/` の中身で、展開すればそのまま版ディレクトリになります。zip の各エントリは時刻を固定して再現可能にし、同じ入力からは同じ SHA-256 が得られます。

manifest は1回の `publish` が所有し、その呼び出しで渡した全プラットフォームをまとめて載せます。同じ版を複数回に分けて `publish` すると後勝ちで asset が置き換わるため、複数プラットフォームは `--binaries windows-x86_64=<dir> --binaries linux-x86_64=<dir>` のように1回で渡します。

`manifest.json` の隣に `manifest.pub`（署名鍵の公開鍵 hex）を置きます。これは照合の補助であり、信頼の根拠にはしません。client はコードに埋め込んだ公開鍵、またはリポジトリに固定した公開鍵で検証します。

## 署名鍵の扱い

- 秘密鍵はリポジトリに含めません。生成は鍵を持つ側だけで行います。
- `keygen` は既存ファイルを上書きしません。秘密鍵は 32 byte seed の hex で、置き場所はリポジトリ外（CI では secret）です。
- 公開鍵（32 byte の hex）だけを配布物とコードに含めます。鍵 ID で複数世代の公開鍵を区別できるようにし、`key_id` と公開鍵の対応を client 側（launcher）に固定します。`manifest.pub` は照合用の写しで、それ単体では信頼の根拠になりません。
- 秘密鍵を失った場合は、新しい鍵 ID で公開鍵を追加します。既存の公開鍵を消すと、その鍵で署名済みの版を検証できなくなります。

CI では `LAPIS_RELEASE_SIGNING_KEY`（hex）を secret として渡します。secret が無い fork では Release への配置を行いません。公開鍵を client へ埋め込む作業は launcher の Issue で行い、それまでは `manifest.pub` と手元の公開鍵で照合します。

## install layout

per-user 配置で、管理者権限を要求しません。初回は zip を展開して launcher を起動する形で、インストーラのウィザードを持ちません。launcher 本体は別 Issue ですが、パスと形式は後付けできないためここで固定します。

```
%LOCALAPPDATA%\Lapis\
  install\
    versions\<version>\          展開済みの版（lapis.exe, lapis-host.exe, ...）
    staging\<version>\           download 済み package の一時置き場
    current.json                 現在の版を指す pointer
  data\
    generation-<N>\              DB・会話・表示状態など、データ世代に属するもの
  credentials\                  Host 接続 credential など、失うと再登録が要るもの
```

- 版は `versions/<version>/` の並置。保持は現行 + 前版の2世代までとし、3世代目以降は切り替え成功後に削除します。切り替え前に前版を消しません。
- `staging/<version>/` は download 中と展開前の package を置く場所です。検証に失敗した package は `versions/` へ移しません。
- `current.json` は launcher が読み書きします。形式は `{"schema": 1, "version": "<version>", "generation": <N>, "activated_at": "<RFC 3339>"}`。書き込みは同一ディレクトリの一時ファイル経由で置換し、途中状態を残しません。
- staging と versions の間の移動は同じ volume 内で行い、`current.json` の置換だけを切り替え点にします。
- `data/` と `credentials/` は install の版に含めず、更新で置き換えません。

## データ世代と credentials

- DB と data は `data/generation-<N>/` で世代番号ごとに分けます。N は 1 から始まります。
- 世代を上げるのは `breaking: true` の release のときだけです。通常の version up（軸A）は同じ世代のまま `versions/` と `current.json` だけを進めます。
- 新しい世代は空から作り、旧世代からの移行処理をその release に含めます。移行に失敗した場合は旧世代を残したまま起動を中止します。
- Host 接続 credential、OIDC の保存 token、Host の identity など「失うと再登録が要るもの」は `credentials/` に置き、データ世代の外に固定します。世代を上げても消しません。
- 現在の実装は `%LOCALAPPDATA%\Lapis` 直下へ設定・会話・hosts を保存しています。上記 layout への移行は launcher と同時に行い、この文書は移行完了までの決定事項として扱います。

## 生成と配置の手順

```powershell
# 1. 鍵（初回だけ。置き場所はリポジトリ外）
python scripts/package/distribution.py keygen --private-key $env:USERPROFILE\lapis-keys\signing.key --public-key $env:USERPROFILE\lapis-keys\manifest.pub

# 2. ビルド済み binary から package と manifest を作り、ローカルへ置く
cargo build --release -p lapis -p host -p web-bridge -p lapis-web
python scripts/package/distribution.py publish --version 0.2.0 --binaries target/release --dest local:dist\v0.2.0 --release-notes-url https://github.com/yuki-net/lapis/releases/tag/v0.2.0 --private-key $env:USERPROFILE\lapis-keys\signing.key

# 3. 配置したものを検証する
python scripts/package/distribution.py verify --root local:dist\v0.2.0 --public-key $env:USERPROFILE\lapis-keys\manifest.pub
```

`publish` は asset → `manifest.pub` → `manifest.json` の順に書き、manifest が揃う前に新しい版を指さないようにします。tag 駆動の CI では `.github/workflows/release.yml` が同じコマンドを `--dest github:<tag>` で実行します。

## 手動復旧

手順は [配布物の手動復旧](../recovery.md) を正本とします。署名鍵は不要で、配布物の `manifest.json` と `manifest.pub` を使います。

1. launcher とアプリを終了する。Host が残っている場合は `scripts/stop_dev_host.ps1` かタスクトレイから止める。
2. `%LOCALAPPDATA%\Lapis\install\current.json` を控える。壊れていれば削除してよい（次回起動時に選び直す）。
3. 戻したい版の package（`lapis-<version>-<platform>.zip`）と、同じ版の `manifest.json` を入手する。
4. `manifest.json` の `assets` から自分のプラットフォームの `sha256` と `size` を取り、zip の SHA-256 と byte 数が一致することを確認する。
   ```powershell
   (Get-FileHash -Algorithm SHA256 .\lapis-0.1.0-windows-x86_64.zip).Hash.ToLower()
   (Get-Item .\lapis-0.1.0-windows-x86_64.zip).Length
   ```
   一致しなければその package は使わない。
5. `%LOCALAPPDATA%\Lapis\install\versions\<version>\` を削除し、zip をそのパスへ展開する。zip の root がそのまま版ディレクトリの中身になる。
6. `current.json` を `{"schema": 1, "version": "<version>", "generation": <N>, "activated_at": "<現在時刻>"}` に書き換える。`generation` は戻す版が使うデータ世代に合わせる。`data/` と `credentials/` は削除しない。
7. launcher を起動し、バージョン表示が戻した版になっていることを確認する。

データ世代を跨いで戻す場合は、旧世代の `data/generation-<N>/` が残っていることだけ確認します。無い場合は旧世代のデータは復元できず、新規 install と同じ状態になります。`credentials/` は世代の外にあるため、この操作で再登録は不要です。

## 検証

`python -m unittest scripts/test_distribution.py` が、test package 2版（0.1.0 → 0.2.0、2版目は `breaking: true`）について次を確認します。

- 生成: zip に必要な実行ファイルだけが入り、同じ入力から同じ SHA-256 になる
- ローカル配置: asset と manifest が同じフォルダに揃い、manifest の SHA-256・size が実ファイルと一致する
- 静的配置: HTTP で配信した同じフォルダを `--root url:` で検証できる
- GitHub Releases 経路: asset が絶対 URL で参照され、upload は manifest を最後に行う
- 改ざん・別鍵・schema 違反・配布フォルダ外参照の拒否

`python scripts/check.py repository` はこのテストを含みます。Python 側の依存は `scripts/package/requirements.txt` で、CI の repository job が同じファイルを入れてから実行します。

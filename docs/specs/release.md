# 配布と更新の内部契約

配布 package・署名付き manifest・インストール layout・データ世代の正本。
利用者・運用者向けの手順は [リリース手順](../releases.md) を参照する。

現時点では launcher と更新 UX は実装していない（Issue #298 の範囲外）。ここに書くのは
生成・配置・検証の契約と、後から変更できない layout の決定である。

## 経路は 1 本

```
package (zip) + manifest.json (+ Ed25519 署名)
        │
        ├─ ローカル配置 (folder / 静的配置)
        └─ GitHub Releases の asset
                    │  同じ形式・同じ検証
                    ▼
  署名検証 → SHA-256 検証 → staging → versions/<version>/ → pointer
```

配置先が違うだけで、manifest の形式・検証・適用は共通とする。ローカル用と Releases 用で
別の manifest や別の読み込み処理を作らない。実装は `scripts/release/` に集約し、
`publish.py` が入り口（ローカル / Releases）だけを分岐させる。

`scripts/release/` は生成・配置・検証の tool である。client 側の更新処理は同じ manifest
schema と検証規則を使うが、launcher 本体と更新 UX はこの Issue では実装しない。

## manifest schema v1

`manifest.json` は初日から次の field を持つ。後から項目を足すと旧 client が解釈できない
ため、追加は schema 改訂として扱う。未知の field と欠けた field はどちらも拒否する。

| field | 型 | 意味 |
| --- | --- | --- |
| `schema` | integer | 常に `1` |
| `version` | string | 例 `0.2.0`。`versions/<version>/` の名前と一致する |
| `breaking` | boolean | 軸B（data 世代が上がる破壊的変更）かどうか |
| `notes_url` | string | release notes の URL。空にしない |
| `generated_at` | string | 生成時刻（UTC、`...Z`） |
| `key_id` | string | 署名鍵の識別子。`trusted-keys.json` の key と対応する |
| `assets` | object | platform 名 → `{ "source", "sha256" }` |
| `signature` | string | base64。下記の署名対象に Ed25519 署名した 64 byte |

`assets.<platform>.source` は manifest と同じ directory からの相対パスか、`https://` の
絶対 URL とする。相対パスは manifest の隣にだけ解決し、`..`・絶対パス・backslash を
拒否する。`sha256` は小文字 16 進 64 桁。

`breaking` は「通常の version up で済むか」「data 世代を上げるか」の判断にだけ使う。
manifest の他の項目から推測しない。

### 署名対象と検証

署名対象は `signature` を除いた manifest を、key 順を sort し区切りを詰めた JSON
（`separators=(",", ":")`、UTF-8）にした bytes である。人が読むための整形は署名対象に
含めない。

検証は次の順で行い、1 つでも失敗したら配置物を採用しない。

1. JSON として読み、field の過不足・`schema`・各型を検証する
2. `key_id` が信頼済み公開鍵 registry にあることを確認する
3. Ed25519 署名を検証する
4. manifest が参照する全 package の SHA-256 を実 file と照合する

信頼の起点は repository の `release/trusted-keys.json` だけとする。配布物に含めた公開鍵を
信頼の起点にしない。配布物の manifest を差し替えても、registry に無い `key_id` は通らない。

## 署名鍵

- 秘密鍵は repository に含めない。生成機だけが持ち、`scripts/release/keys.py` の
  `keygen` で file に書き出す。file は `--key` か環境変数 `LAPIS_RELEASE_SIGNING_KEY`
  （鍵 file の path、CI では中身を file に復元した path）で渡す。
- 秘密鍵を package・manifest・Release asset・artifact に含めない。`build` の出力は
  manifest と package だけである。
- 公開鍵だけを `release/trusted-keys.json` に置く。key の追加・失効はこの file の変更として
  レビューする。
- CD では GitHub Actions の secret `LAPIS_RELEASE_SIGNING_KEY` から鍵 file を復元する。
  secret が無ければ release workflow は失敗させ、鍵なしの署名に fallback しない。

## インストール layout

per-user 配置とし、管理者権限を要求しない。初回は package を置いて launcher を起動する
形で、インストーラのウィザードを持たない。launcher 本体は別 Issue。

```
<install root>/
  current.json           現在 version を指す pointer
  versions/
    <version>/           展開済み package + その出所の manifest.json と zip
  staging/               download 済みで未検証・未適用の package
  data/
    <generation>/        DB / 保存 data
  credentials/           data 世代の外。失うと再登録が要る credential
```

- 並置型: 各 version は `versions/<version>/` に独立して置く。version 間で file を共有しない。
- 保持は現行 + 前版の 2 世代まで。新しい version の適用後に、それより古い directory を
  削除する。削除するのは directory 名と manifest の `version` が一致する導入済み version
  だけであり、手動復旧で退避した `0.3.0.broken` のような directory は残す。
  `staging/` は保持数に数えない。
- pointer は `<install root>/current.json` で、`{"schema": 1, "version": "<version>"}`。
  書き込みは一時 file に書いて `os.replace` する原子的な置き換えとし、途中の値を見せない。
- `versions/<version>/` は一時 directory に展開してから rename する。展開が途中で失敗した
  package を pointer の指す先にしない。
- package は zip の root を `versions/<version>/` の直下に展開する。zip member の絶対パス、
  `..`、drive 指定、layout の予約 name（`current.json`、`manifest.json`）を拒否する。
  `zipfile` は member 名の `\` を `/` に正規化して読むが、`\` を含む名前も拒否する。
- layout の定数は `scripts/release/layout.py` を正とする。launcher はここを import する
  代わりに同じ値を使うが、version の探索や保持数を独自に決めない。

## データ世代

- DB / data は `<install root>/data/<generation>/` に置き、世代番号で分ける。
- 世代を上げるのは `breaking: true` の release（軸B）のときだけ。軸A（通常の version up）は
  同じ世代の directory を使い続ける。
- 新しい世代を用意する場合も、旧世代の directory を自動削除しない。世代の切り替えと旧世代の
  削除は別の操作とする。
- 失うと再登録が要るもの（Host 接続 credential、OIDC token、DPAPI 保護 profile など）は
  data 世代の外、`credentials/` に置く。世代交代で credential を失わせない。
- credentials の暗号化・保存形式は各機能の所有（[認証とセッション](auth.md)）であり、
  この layout は置き場所だけを決める。

## GitHub Releases への配置制約

- tag は人の操作で作り、`scripts/release` からは作成しない。
  `publish --github` は既定で `gh release upload --clobber` だけを行う。
- `--execute` を付けない限り dry-run で、GitHub を読まず・書かない。実行時も tag と
  Release の存在を確認し、無ければ失敗する。Release の作成は `--create` を明示したとき
  だけ行う。
- CI（`.github/workflows/release.yml`）は push 済み tag に対して、`--create` 付きで
  Release の作成または asset の差し替えを行う。作成時も `gh release create --verify-tag`
  を使い、tag を新規作成しない。

## 検証

- `python -m scripts.release verify --source <dir>` は署名・SHA-256 を検証する。
- `python scripts/check.py repository architecture` が `scripts/release/test_release.py` を
  含めて実行する。Ed25519 は RFC 8032 7.1 のテストベクタで固定する。
- Release を実際に作る検証を行わない。GitHub 側は fake remote と runner に差し替える。

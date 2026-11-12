# 配布と更新の内部契約

配布 package・署名付き manifest・インストール layout・データ世代の正本。
利用者・運用者向けの手順は [リリース手順](../releases.md) を参照する。

現時点では launcher 本体までを実装している。アプリ側の更新確認・download・世代切替の
同意画面（更新 UX）は未実装である。ここに書くのは生成・配置・検証の契約と、後から
変更できない layout・launcher の起動前提の決定である。

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

`scripts/release/` は生成・配置・検証の tool である。launcher（`clients/desktop/launcher/`）
は同じ manifest schema と検証規則を使い、staging 済み package の適用と本体の起動だけを行う。
manifest の取得・download・更新の同意画面といった変わりうる処理はアプリ側に置き、
launcher には置かない。

## manifest schema v1

`manifest.json` は初日から次の field を持つ。後から項目を足すと旧 client が解釈できない
ため、追加は schema 改訂として扱う。未知の field と欠けた field はどちらも拒否する。

| field | 型 | 意味 |
| --- | --- | --- |
| `schema` | integer | 常に `1`。JSON の `true` など bool は受理しない |
| `version` | string | 例 `0.2.0`。`versions/<version>/` の名前と一致する |
| `breaking` | boolean | 軸B（data 世代が上がる破壊的変更）かどうか |
| `notes_url` | string | release notes の URL（scheme が `https` / `http` で host が必須。空にしない） |
| `generated_at` | string | 生成時刻（UTC、`...Z`） |
| `key_id` | string | 署名鍵の識別子。`trusted-keys.json` の key と対応する |
| `assets` | object | platform 名 → `{ "source", "sha256" }` |
| `signature` | string | base64。下記の署名対象に Ed25519 署名した 64 byte |

`assets.<platform>.source` は manifest と同じ directory からの相対パスとする。schema v1 は
`https://` の asset 取得を実装していないため URL を許可しない。`build` は常に package の
file 名（相対パス）だけを書き、検証側も URL を明示的に拒否する。相対パスは manifest の隣に
だけ解決し、`..`・絶対パス・backslash を拒否する。`sha256` は小文字 16 進 64 桁。

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
  `keygen` で file に書き出す。file は作成時点で排他的に mode `0600` で作り、既存 file は
  上書きしない。権限設定に失敗した場合は鍵 file を残さず例外を返す。file は `--key` か
  環境変数 `LAPIS_RELEASE_SIGNING_KEY`（鍵 file の path、CI では中身を file に復元した
  path）で渡す。
- 配布物と署名の検証に使う公開鍵のうち、恒等元・小位数点（位数が 8 以下）の点は鍵として
  拒否する。このような点を受けると `s=0` などの署名が任意の message で通る。
- 秘密鍵を package・manifest・Release asset・artifact に含めない。`build` の出力は
  manifest と package だけである。
- 公開鍵だけを `release/trusted-keys.json` に置く。key の追加・失効はこの file の変更として
  レビューする。
- CD では GitHub Actions の secret `LAPIS_RELEASE_SIGNING_KEY` から鍵 file を復元する。
  secret が無ければ release workflow は失敗させ、鍵なしの署名に fallback しない。

## インストール layout

per-user 配置とし、管理者権限を要求しない。初回は package を置いて launcher を起動する
形で、インストーラのウィザードを持たない。launcher は `clients/desktop/launcher/`。

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
  `staging/` は保持数に数えない。同じ version を `--replace` で入れ直した場合は pointer から
  前版を知れないため、導入済みの version を新しい順に補って 2 世代を割らない。
- pointer は `<install root>/current.json` で、`{"schema": 1, "version": "<version>"}`。
  書き込みは一時 file に書いて `os.replace` する原子的な置き換えとし、途中の値を見せない。
  壊れた pointer を勝手に作り直さない。`install` は pointer を展開前に読むため、破損時は
  何も展開せず停止する。
- `activate` は pointer を読まずに書き直す。破損した pointer からの復旧経路はこれだけで、
  対象 version の manifest・package・展開済み file を検証できた場合に限り成功する。
- `versions/<version>/` は一時 directory に展開してから rename する。展開が途中で失敗した
  package を pointer の指す先にしない。
- package は zip の root を `versions/<version>/` の直下に展開する。zip member の絶対パス、
  `..`、drive 指定、layout の予約 name（`current.json`、`manifest.json`）を拒否する。
  `zipfile` は member 名の `\` を `/` に正規化して読むため、正規化前の名前
  （`ZipInfo.orig_filename`）も検査し、`\` を含む名前を拒否する。
- `install` は manifest が参照する全 platform の package を `versions/<version>/` に保持する。
  `verify_distribution` が全 package を要求するため、選択した platform だけを残さない。
- pointer の読み取りから版本の置換、pointer 更新、prune までを root 単位で排他する。
  `activate` も同じ排他に入る。並行する install 同士が互いの前版を削除して pointer の
  参照先を失うのを防ぐ。
- 検証は複写後の staged directory に対して行い、展開もその検証済み package から行う。
  展開対象は複写後の再検証結果から選び直し、初回検証の manifest と食い違う package を
  展開しない。展開後は展開済み file を package の内容と照合し、不一致なら pointer を
  更新しない。symlink は package に含まれないため、展開先にあれば不一致として拒否する。
- `activate` も保存済み ZIP だけでなく展開済み file を照合する。実行 file だけを書き換えた
  version は復旧先として使わない。install は選択した platform だけを展開するため、照合は
  manifest の各 platform の package に対して行い、どれか 1 つでも展開内容と一致すれば
  その platform の version として扱う。1 つも一致しなければ検証失敗とする。
- layout の定数は `scripts/release/layout.py` を正とする。launcher はここを import する
  代わりに同じ値を使うが、version の探索や保持数を独自に決めない。

## launcher

`lapis-launcher.exe` は起動時に staging の適用と本体の起動だけを行う。ネットワーク・
manifest の解釈・更新モードの設定を持たず、変更のたびに再配布が要る処理を置かない。

1. `<install root>/staging/<version>/` を新しい順に見て、適用できる最初の 1 つを
   `versions/<version>/` へ入れる。署名・SHA-256・展開内容の照合は `install` と同じ規則で
   行い、一致しなければ pointer を変えない。
2. `current.json` の version にある `lapis{,.exe}` を、受け取った引数で起動する。

- install root は `--install-root`、環境変数 `LAPIS_INSTALL_ROOT`、`~/.lapis` の順に決める。
  `--install-root` は本体へ渡さない。
- 更新の失敗・staging の不在・staging の破損・`breaking: true` の候補は、起動を妨げない。
  適用しなかった staging は削除せず残し、pointer の version で起動する。適用できた
  staging は同じ package を再度展開しないよう片付ける。
- `breaking: true` の適用は同意を伴う操作であり、launcher では行わない。同意画面と
  data 世代の切替はアプリ側の範囲とする。
- pointer の version と同じか古い候補は適用しない。比較は semver で行い、解釈できない
  形式では一致しない限り適用する。
- pointer が無い・壊れている・指す先の manifest が version と一致しない場合は、pointer を
  作り直さず起動を止める。復旧は `activate` か `install` の手順で行う。
- 適用中の排他は `<install root>/.layout.lock` で `install` と同じく root 単位とする。
  取得できなければ今回の適用を見送り、起動は続ける。
- 信頼済み公開鍵 registry は launcher に埋め込む。鍵の追加・失効は launcher の再配布を
  伴うが、配布物に含めた公開鍵を信頼の起点にはしない。

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
- `--execute` では公開先 tag が `v<manifest の version>` と一致することを remote 操作の
  前に確認する。`--clobber` で別 version の成果物を既存 tag の Release へ上書きしない。
- repo の照会は slug（`owner/repo`）を受けるが、`git ls-remote` へは
  `https://github.com/<owner>/<repo>` に変換して渡す。
- CI（`.github/workflows/release.yml`）は push 済み tag に対して、`--create` 付きで
  Release の作成または asset の差し替えを行う。作成時も `gh release create --verify-tag`
  を使い、tag を新規作成しない。手動実行（`workflow_dispatch`）でも、入力した tag の
  revision を checkout してから生成・署名する。

## 検証

- `python -m scripts.release verify --source <dir>` は署名・SHA-256 を検証する。
  install root を渡した場合は pointer の version について、保存済み package に加えて
  展開済み file も ZIP の内容と照合する。launcher の起動前提（実行 file が署名済み package
  の内容と一致する）を verify でも確認できる状態に保つ。
- `python scripts/check.py repository architecture` が `scripts/release/test_release.py` を
  含めて実行する。Ed25519 は RFC 8032 7.1 のテストベクタで固定する。
- launcher は `cargo test -p lapis-launcher` で、`scripts.release` が生成・署名した fixture
  配布物を検証する。Python 側の正規化 bytes・署名・layout の規則と Rust 実装が食い違うと
  落ちる。fixture を更新するときは `scripts.release` で作り直す。
- Release を実際に作る検証を行わない。GitHub 側は fake remote と runner に差し替える。

# リリース手順

配布 package と署名付き manifest の生成・配置・検証・手動復旧の手順。
内部契約（manifest schema、layout、データ世代）は [配布と更新の内部契約](specs/release.md) を
正とする。

## 前提

- Python 3.10 以上。`scripts/release` は標準 library だけで動く。
- 署名鍵 file は生成機だけに置く。repository へ commit しない。
- tag と Release の作成は人の操作として行う。tool は tag / Release を作らない。

## 1. 署名鍵

初回だけ生成する。出力される `public` を `release/trusted-keys.json` へ追加し、
秘密鍵 file は repository の外へ保管する。

```sh
python -m scripts.release keygen --key-id lapis-release --out ~/.lapis/release-key.json
```

`release/trusted-keys.json`:

```json
{ "schema": 1, "keys": { "lapis-release": "<keygen が表示した public>" } }
```

以後の生成では鍵 file を `--key` で渡す。環境変数 `LAPIS_RELEASE_SIGNING_KEY` に
鍵 file の path を設定してもよい。CI では secret `LAPIS_RELEASE_SIGNING_KEY` に鍵 file の
中身を入れ、workflow が一時 file に復元する。secret が無い場合 workflow は失敗する。

`release/trusted-keys.json` に公開鍵が無い `key_id` の manifest は検証で拒否される。
release workflow も同じ registry を使うため、公開鍵を登録するまで CI の Release job は
失敗する（未登録の鍵で署名された成果物を通さないための意図的な失敗）。

## 2. package と manifest の生成

platform ごとに展開後の file を directory へまとめてから生成する。`--platform` は
`<platform 名>=<directory>` で、複数指定できる。

```sh
python -m scripts.release build \
  --version 0.2.0 \
  --platform windows-x86_64=target/package/lapis \
  --out dist \
  --notes-url https://github.com/yuki-net/lapis/releases/tag/v0.2.0

python -m scripts.release verify --source dist
```

`dist/` に `manifest.json` と `<name>-<version>-<platform>.zip` ができる。`--breaking` は
data 世代を上げる release（軸B）のときだけ付ける。

## 3. 配置

### ローカル（folder / 静的配置）

```sh
python -m scripts.release publish --source dist --local /path/to/public/releases/v0.2.0
```

`dist/` と同じ形式（manifest + zip）がそのまま複写される。静的サーバーの場合は、この
directory をそのまま公開する。`manifest.json` の `assets.<platform>.source` は相対パスの
ままなので、manifest と同じ directory に package が並んでいる必要がある。

### GitHub Releases

tag と Release を先に作り、asset を配置する。

```sh
# dry-run。実行される command を確認するだけで、GitHub を読まず・書かない
python -m scripts.release publish --source dist --github --tag v0.2.0

# 実行。既存 Release の asset を差し替える（tag と Release は作らない）
python -m scripts.release publish --source dist --github --tag v0.2.0 --execute
```

`--execute` は remote に tag と Release が既にあることを確認してから
`gh release upload --clobber` を実行する。どちらかが無ければ失敗する。

CI では tag を push すると `.github/workflows/release.yml` が Release を作成・更新する。
初回の Release 作成は workflow から `--create` を付けて行うが、CLI は `gh release create`
に `--verify-tag` を必ず付け、push 済みの tag を検証するだけで新規作成しない。

## 4. インストール layout への配置

```sh
python -m scripts.release install \
  --source dist \
  --root ~/.lapis \
  --version 0.2.0 \
  --platform windows-x86_64
```

`~/.lapis/versions/0.2.0/` に展開し、`~/.lapis/current.json` を 0.2.0 に更新する。
現行 + 前版の 2 世代だけを残す。検証は次で行う。

```sh
python -m scripts.release verify --source ~/.lapis
```

## 5. 手動復旧

更新が壊れたときは、手元にある検証済み package で version を差し替える。

1. アプリを終了する。launcher と Host を終了し、`versions/<version>/` を掴んでいる
   プロセスが無いことを確認する。
2. 現在の状態を控える。

   ```sh
   python -m scripts.release verify --source ~/.lapis
   cat ~/.lapis/current.json
   ```

3. 戻したい version の配置物（manifest + zip）を用意する。手元の `dist/` でも、配置済みの
   ローカル folder でも、Release から download したものでもよい。形式は同じである。
4. 対象 version の directory を退避してから配置し直す。

   ```sh
   mv ~/.lapis/versions/0.3.0 ~/.lapis/versions/0.3.0.broken
   python -m scripts.release install \
     --source /path/to/verified-dist \
     --root ~/.lapis \
     --version 0.2.0 \
     --platform windows-x86_64
   ```

   `install` は署名と SHA-256 を検証してから展開する。検証に失敗した package は
   pointer を書き換えない。同じ version を入れ直す場合は検証後に `--replace` を明示する
   （`--replace` なしでは既存 directory を上書きしない）。
5. launcher を起動し、`current.json` の version で起動することを確認する。
6. `~/.lapis/versions/0.3.0.broken` は原因確認が済むまで残す。`data/` と `credentials/` は
   この操作では触らない。

前版の directory が無事に残っている場合は、再取得せず pointer だけを戻せる。この経路も
`versions/<version>/` の manifest と package を検証してから pointer を書き換える。

```sh
python -m scripts.release activate --root ~/.lapis --version 0.2.0 --platform windows-x86_64
```

`current.json` が壊れて JSON として読めない場合、`verify` は失敗する。`current.json` を
手で書き換えず、正しい version を `install` で入れ直す。`current.json` を削除すると
未導入状態として扱われ、`versions/` は残るが起動先が無くなる。

## 6. やってはいけないこと

- 秘密鍵 file、seed、token を repository・PR・Release asset へ置かない。
- 検証を通さずに `versions/<version>/` や `current.json` を手で書き換えない。
- data 世代を通常の version up で上げない。`data/<generation>/` と `credentials/` を
  手動で削除しない。
- テストや手元確認で実際の tag / Release を作らない。Releases への配置確認は
  `--github` の dry-run か `--local` を使う。

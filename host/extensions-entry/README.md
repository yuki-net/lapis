# extensions-entry（拡張の入口）

導入済み拡張の目録・状態、`.lapis` package の読み込み、package・拡張データの置き場所を
持つ crate。配布や販売を意味する `store` ではなく、拡張の入口としての最小実装。

Wasm ランタイム上での拡張実行はこの crate の対象外（将来段階）。

## 構成

- `catalog.rs`：`ExtensionCatalog`。目録の読み書き、登録、有効状態の変更、配置 directory の解決。
- `state.rs`：`ExtensionState`。`installed` / `enabled` / `incompatible` を持ち、
  `enabled` は `installed` を含意し、`incompatible` は `installed` と同時に立たない。
- `id.rs`：`ExtensionPackageId`。配置 directory 名にそのまま使える ID だけを許す。
  大文字・`/`・`\`・`:`・末尾 `.`（Windows が path 要素から取り除くため `a.` と `a`
  が衝突する）・Windows の予約デバイス名（`nul`、`nul.txt` など）を拒否する。
- `version.rs`：`ExtensionVersion`。表示と比較にだけ使い、semver の解釈はしない。
- `compatibility.rs`：`ContractRequirement` / `HostContracts` / `Incompatibility`。
  manifest が要求する契約と版の範囲を Host の提供値と突き合わせる。
- `signature.rs`：`TrustedKeys`。リリース署名（`scripts/release`、
  `clients/desktop/launcher`）と同じ Ed25519 の規則で manifest を検証する。
- `package.rs`：`ExtensionLoader`。zip + manifest の読み込み、SHA-256 照合、展開、
  目録への登録、package の取り除き。
- `declaration.rs`：`ExtensionDeclarations`。目録を Client へ見せる宣言へ集約する。
- `journal.rs`：`PendingInstall`。導入・除去の未完了を記録し、起動時に回復する。

## 目録

`<state>/extensions/catalog.json`（`schema: 2`）が宣言の正本である。1 件は
`id` / `version` / `installed` / `enabled` / `incompatible` の平坦な object とする。
`incompatible` を欠く `schema: 1` は「非互換でない」として読む。書き換えは常に現在の
schema で行い、旧い入口には未知の schema として読み込みを拒否させる。これにより、
新旧の入口が同じ目録を黙って壊し合わない。

登録は「配置を作る → 目録を書く」順で、配置を作れない場合は目録を書かない。
`create_dir_all` の成功だけでは判断せず、作成後に directory の実在を確かめる
（Windows の予約デバイス名は成功を返して何も作らないことがある）。

保存済みの ID が現在の規則に合わない場合、その項目だけを捨てずに目録全体の読み込み
エラーとする。file は上書きしないため、規則に合う形へ直すか退避するまで元の値が残る。

## `.lapis` package の読み込み

package の種別は manifest の `kind`（`extension` / `host`）が決める。拡張子や招待
ファイル（`.lapis-host`）の名前では区別しない。

読み込みは次の順に行い、途中で失敗したら配置を変えない。

1. zip として読み、member 名・重複・上限（file 数・サイズ）を検証する
2. `manifest.json` の未知・欠落 field、型、値の範囲を検証する
3. `key_id` が信頼済み registry にあることを確認し、Ed25519 署名を検証する
4. manifest が宣言する全 file の SHA-256 を zip の内容と照合し、宣言外の file を拒否する
5. `requires` の契約と版の範囲を Host の提供値と照合する
6. 一時 directory へ書き、目録を書けたら実体を置き換える（旧版は退避してから消す）

目録の書き込みを実体の置き換えより先に行う。逆順にすると、目録を書けなかったときに
「実体はあるが宣言は未導入」の状態を残す。置き換えに失敗した場合は目録を元の宣言へ戻し、
退避した旧版を元の場所へ戻す。

manifest の形は次のとおり。

```json
{
  "schema": 1,
  "kind": "extension",
  "id": "dev.example.terminal",
  "version": "0.2.0",
  "requires": [
    { "contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0" }
  ],
  "files": [
    { "path": "extension.wasm", "sha256": "<小文字 16 進 64 桁>" }
  ],
  "key_id": "lapis-release",
  "signature": "<base64 の 64 byte>"
}
```

署名対象は `signature` を除いた manifest を key 順に並べ、区切りを詰めた UTF-8 の
JSON bytes である。`scripts/release/manifest.py` の `canonical_bytes` と同じ。

## 非互換・署名不正の扱い

- 署名が一致しない package は目録にも載せない。manifest の `id` すら署名で裏付けられて
  いないため、宣言として採用しない。
- 互換性がない package は展開せず、目録に `incompatible` として登録する。旧版の実体は
  取り除き、宣言と置いてある package を食い違わせない。拡張データは残す。ただし導入済みの
  版が動いている場合は、その宣言と実体を残す（入れようとした版の都合で動いている版を
  壊さない）。
- `incompatible` の拡張は有効にできない。`host.extensions.list` は `incompatible` を
  宣言に含めるので、Client は「未導入」と「非互換」を区別して表示できる。

## 有効化・無効化

目録の状態だけを変え、package の実体は動かさない。反映は入口の再起動で行う。同じ状態への
再登録は目録の file を書き換えない。導入済み package を取り除いても
`<state>/extensions/data/<id>/` は残し、再導入で引き継ぐ。取り除きでも実体は先に退避し、
目録を書けたときだけ消す（目録を書けなければ実体を戻す）。

## 中断時の回復

目録（宣言）と `installed/<id>/`（実体）は別の file なので、2 つの書き込みの間で Host が
終了すると食い違う。`journal.rs` は操作の開始時に「前の宣言」を `.swap/../journal.json`
へ記録し、完了時に消す。起動時（`recover_pending_installs`）に残った記録と実体を見て、
完了または取り消しを行う。

- 退避だけが残っている（`swap` の前後で落ちた）: 旧版を元の場所へ戻し、宣言も操作前へ戻す。
- 退避も実体も無い（初回導入の途中で落ちた）: 宣言を取り除く。
- `.swap` と `.staging-*` の記録外の残骸も片付ける。

記録に無い宣言には触れない。宣言だけを登録する段階（PR1 の配置解決）を、回復が
書き換えないため。

## データの置き場所

```
<host state>/
  extensions/
    catalog.json              目録（宣言の正本）
    installed.lock            読み込みの排他
    journal.json              未完了の導入・除去の記録
    .swap/<id>/               置き換え中の旧版の退避先（完了時に消す）
    installed/<id>/           導入済み package の展開先
    data/<id>/                拡張データの正本
```

状態は Host のデータ領域にだけ置き、Client は `host.extensions.list` で受け取った
宣言を表示にだけ使う。Client 側の保持は再取得可能な cache とする。

配置の判断理由は [責務と依存方向](../../docs/specs/architecture.md#拡張の入口) を参照。

## 検証

- `cargo test -p extensions-entry`：目録の冪等性・破損時の非上書き・並行登録・配置解決、
  署名の正規化・registry、署名済み package の正常系、非互換 package、署名不正 package、
  改竄 file、宣言外 file、`kind: host` の拒否、有効化・無効化・除去。
- `cargo test -p host --test extensions`：Host の state directory 配下の目録、非互換の
  宣言、再起動後の反映、署名不正 package の拒否。

# extensions-entry（拡張の入口）

導入済み拡張の目録・状態と、package・拡張データの置き場所を持つ crate。配布や販売を
意味する `store` ではなく、拡張の入口としての最小実装。

`.lapis` package の読み込み・互換照合・署名検証・Wasm 実行はこの crate の対象外。

## 構成

- `catalog.rs`：`ExtensionCatalog`。目録の読み書き、登録、配置 directory の解決。
- `id.rs`：`ExtensionPackageId`。配置 directory 名にそのまま使える ID だけを許す。
  大文字・`/`・`\`・`:`・末尾 `.`（Windows が path 要素から取り除くため `a.` と `a`
  が衝突する）・Windows の予約デバイス名（`nul`、`nul.txt` など）を拒否する。
- `version.rs`：`ExtensionVersion`。表示と比較にだけ使い、semver の解釈はしない。
- `state.rs`：`ExtensionState`。`installed` / `enabled` を持ち、`enabled` は
  `installed` を含意する。

登録は「配置を作る → 目録を書く」順で、配置を作れない場合は目録を書かない。
`create_dir_all` の成功だけでは判断せず、作成後に directory の実在を確かめる
（Windows の予約デバイス名は成功を返して何も作らないことがある）。

保存済みの ID が現在の規則に合わない場合、その項目だけを捨てずに目録全体の読み込み
エラーとする。file は上書きしないため、規則に合う形へ直すか退避するまで元の値が残る。

## データの置き場所

```
<host state>/
  extensions/
    catalog.json              目録（宣言の正本）
    installed/<id>/           導入済み package の展開先（読み込み段階が作る）
    data/<id>/                拡張データの正本
```

状態は Host のデータ領域にだけ置き、Client は `host.extensions.list` で受け取った
宣言を表示にだけ使う。Client 側の保持は再取得可能な cache とする。

配置の判断理由は [責務と依存方向](../../docs/specs/architecture.md#拡張の入口) を参照。

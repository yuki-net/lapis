# extensions-entry（拡張の入口）

導入済み拡張の目録・状態と、package・拡張データの置き場所を持つ crate。配布や販売を
意味する `store` ではなく、拡張の入口としての最小実装。

`.lapis` package の読み込み・互換照合・署名検証・Wasm 実行はこの crate の対象外。

## 構成

- `catalog.rs`：`ExtensionCatalog`。目録の読み書き、登録、配置 directory の解決。
- `id.rs`：`ExtensionPackageId`。配置 directory 名にそのまま使える ID だけを許す。
- `version.rs`：`ExtensionVersion`。表示と比較にだけ使い、semver の解釈はしない。
- `state.rs`：`ExtensionState`。`installed` / `enabled` を持ち、`enabled` は
  `installed` を含意する。

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

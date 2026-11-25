# lints — 一度直した指摘を、次からは機械で止める

レビューで見つけた実装ミスを、同じクラスごと再発しないようにするための置き場。
**1 ルール = 1 カタログ**で、実装・テスト・fixture の 3 つを必ず揃える。

## どの手段で止めるか（上から順に強い）

| `remedy` | 手段 | 例 |
| --- | --- | --- |
| `design` | 型・境界・データ構造で、そのバグのクラス自体を消す | 不正な状態を表現できない型 |
| `static` | 静的解析で検出する（clippy / 自作検査） | 禁止 API、依存方向、行数 |
| `test` | 振る舞いの回帰をテストで止める | 並行性、状態遷移、タイムアウト |
| `schema` | 契約・データの不変条件をスキーマで検査する | wire DTO、manifest |
| `policy` | 人間の確認を必須にする | 保護パス（`AGENTS.md`、`lints/`、CI 定義） |
| `prose` | 文章で伝える（最後の手段） | 判断基準、命名の好み |

**設計で消せるものを lint で覆わない。** `remedy` が `design` 以外のルールは、
カタログに「なぜ設計で消せないか」（`why_not_design`）を書く。

## 一覧

| id | remedy | 対象 | 実装 |
| --- | --- | --- | --- |
| [file-length](catalog/file-length.md) | design | rust | `scripts/check_architecture.py` |
| [protected-paths](catalog/protected-paths.md) | policy | repository | `scripts/ci/policy.py` |

## 構成

```
lints/
  README.md                 # このファイル（索引）
  catalog/<id>.md           # 1 ルール = 1 ファイル（先頭に front matter）
  baseline.json             # 行数上限の既存違反（ラチェット）
  fixtures/<id>/fail/       # 元のバグを再現する入力。必ず違反する
  fixtures/<id>/pass/       # 正しい入力。必ず違反しない
```

`baseline` を JSON にしているのは、手元の Python 3.10 でも `check_architecture.py`
を動かすため（`tomllib` は 3.11 以降）。既存の `scripts/architecture-exceptions.json`
と同じ形式に揃えている。

## ルールを足す

1. `lints/catalog/<id>.md` を作る。front matter は次の必須項目を埋める
   ```markdown
   ---
   id: <id>
   remedy: design | static | test | schema | policy | prose
   why_not_design: design 以外のとき必須
   scope: rust | python | repository | ...
   implementation: <module>:<qualname>
   test: <module>:<qualname>
   added: YYYY-MM-DD
   ---
   ```
2. `lints/fixtures/<id>/fail/` と `pass/` を置く（`prose` 以外は必須）
3. 実装とテストを足す
4. `lints/README.md` の一覧に 1 行足す
5. `python -m unittest scripts.ci.test_catalog` で三者一致を確認する

`test_catalog.py` が検査するもの:

- front matter の必須項目と `remedy` の値
- `id` とファイル名の一致、README への掲載
- `implementation` / `test` の参照先が実在すること
- fail fixture が実際に違反し、pass fixture が違反しないこと
- `lints/baseline.json` が現状と一致すること

fixture が無いルールは、半年後に静かに発火しなくなる。**fail が落ちることを
CI で証明できないルールは入れない。**

## 保護パス

`lints/` 自体が保護パス。ルールを弱める変更（baseline の引き上げ、fixture の削除、
検査の無効化）は `Policy` が notice で通知し、`@yuki-net/reviews` の承認が必要になる。

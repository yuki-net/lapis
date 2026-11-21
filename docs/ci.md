# CI の検証

CI で走る検証は、ローカルでも同じものが同じコマンドで走ります。GitHub Actions は最後の確認として使う。

## 何が正本か

| 場所 | 所有するもの |
| --- | --- |
| `scripts/ci/routes/<route>.py` | **route の検証内容の正本**。route 名・走らせる OS（`runs_on`）・タイムアウト・必要な toolchain・step 列（1 route 1 ファイル） |
| `scripts/ci/model.py` | `Step` / `Route` の型、runner 名と OS 表示名、toolchain などの固定値 || `scripts/ci/routes/__init__.py` | route の集合と実行順（`ROUTES` / `ORDER`）、job 名の組み立て |
| `scripts/ci/routes/platforms/` | OS ごとの差分 step（`<route>_<os>.py` を置くだけ。無い OS のファイルは作らない） |
| `scripts/ci/paths.py` | **変更 path → route の対応の正本**。1 つの path が複数 route を要求する関係を持つ |
| `scripts/ci/jobs.py` | 上記の再エクスポート（従来の import 先を保つ） |
| `scripts/ci/plan.py` | 上 2 つを繋ぎ、実行すべき route を route × OS の matrix にする |
| `scripts/ci/run.py` | 1 route をローカル / GitHub 共通で実行 |
| `scripts/ci/report.py` | ログ装飾・失敗注釈・実行サマリ |
| `scripts/ci/gate.py` | plan と matrix の結果から CI の成否を決める |
| `scripts/ci/measure.py` | route の step 別所要時間を測る |
| `scripts/ci/watch.py` | route × OS を並列に実行し、進捗を 1 画面で描く |
| `scripts/ci/state.py` | 実行中の route の状態（`.ci/state/<job>.json`） |
| `.github/workflows/ci.yml` | `plan` / `ci`（matrix）/ `gate` の 3 job のみ。step 列は持たない |

`.github/workflows/ci.yml` に step を書き足さないこと。検証を追加するときは `scripts/ci/routes/<route>.py` を、path の振り分けを変えるときは `scripts/ci/paths.py` を編集する（`scripts/ci/plan.py` のテストが YAML への直書きを検出して落とす）。

## route と OS

ファイル名・route key・`name` に OS は書かない。走らせる OS は `Route.runs_on` に持ち、GitHub の job 名（`Host / Windows`）は `runs_on` から組み立てる。検証対象が runner の OS と違う場合（`Mobile / Android`）だけ `Route.target` で対象を表示する。

- OS を増やすときは `runs_on` に足すだけ。ファイル名も `paths.py` も変えなくてよい
- OS ごとに step が違う場合は `scripts/ci/routes/platforms/<route>_<os>.py` に `STEPS` を置く。**ファイルを置くだけで効き、route ファイルは編集しない**。`python -m scripts.ci.run --list` の `platform` 列で読み込まれているか確認できる
- `runs_on` に無い OS では job 自体が生まれない。**skip で緑にならない**
- runner を絞りたいときは `python -m scripts.ci.plan --only-os linux`。`ci.yml` も route 定義も変えずに、その OS の job だけを matrix に入れられる

## route 一覧

`python -m scripts.ci.run --list` で確認できる。

| route | `runs_on` | 主な検証 |
| --- | --- | --- |
| `repository` | linux | `scripts/check.py repository architecture`（CI・拡張・開発コマンドの Python テスト、アーキテクチャ検査） |
| `dev_commands` | windows | 開発起動コマンド・`dev-session` の整形とテスト |
| `rust_core` | linux | backend 全体の architecture / format / test / lint と外部拡張 component |
| `host` | windows | Host の architecture / format / test / lint / build と承認済み拡張 PTY・Web 公開 |
| `desktop` | windows | Desktop の architecture / format / test / lint / build と GPUI patch 検証 |
| `mobile` | linux | KMP のユニットテストと Android ビルド（job 名は `Mobile / Android`） |
| `web` | linux | Web のビルドと接続処理テスト |

GitHub の job 名は `name` と `runs_on`（または `target`）から作る（例 `Host / Windows`, `Mobile / Android`）。

## ローカルで回す

```powershell
python -m scripts.ci.run --list                  # route / runs_on / 前提を確認
python -m scripts.ci.run repository              # route 全体
python -m scripts.ci.run rust_core --step lint   # step を絞る
python -m scripts.ci.run desktop --json          # エージェント向け（ログを抑制）
python -m scripts.ci.run host --os windows       # CI と違う OS として実行する
python -m scripts.ci.measure host                # step 別の所要時間を並べる
```

- 成功した step の出力は捨てる。失敗した step だけ末尾 40 行を表示し、全文は `.ci/logs/<route>/<step>.log` に残る
- `--json` は `{route, name, ok, steps:[{name, ok, seconds}]}` だけを返す。ログを読まずに成否を判断できる
- 前提（cargo / node / wasm-bindgen / java / gradle）が無い route は実行せず理由を表示する。`--strict` を付けると失敗扱いになる
- 手元の OS が `runs_on` に無い route は**実行せず失敗する**。CI で走らない検証を手元で緑にしても意味がないため。`--os` を明示したときだけ実行する
- `--os` は CI の matrix からも渡す。ローカルで別 OS の検証を試すときにも使う
- step の上限は `Step.timeout_minutes`、無指定なら `Route.timeout_minutes` から 10 分（後処理のキャッシュ保存分）を引いた値

## まとめて回す（GitHub と同じ並列）

```powershell
python -m scripts.ci.watch                  # 変更 path から選ぶ
python -m scripts.ci.watch --no-diff        # 全 route
python -m scripts.ci.watch host desktop     # route を指定
python -m scripts.ci.watch --jobs 2         # 同時実行数を絞る
```

route × OS の job を GitHub の matrix と同じように並列で起動し、1 画面に進捗を出す。

```
CI

OK Host / Windows       [########--] 14/18   12m03s  Lint Host
>> Desktop / Windows    [#####-----]  9/18    7m41s  Test Desktop      no output 12s
.. Web                  [----------]  0/3      -      (waiting)
```

- 実行中は**最終出力からの経過**を出すので、止まっていないか分かる
- `Ctrl+C` で起動した job を子孫ごと止める（終了コード 130）
- `OK` は緑、`NG` は赤、`>>` は黄、`..` は待機。記号は ASCII なので cp932 の端末でも崩れない（色は TTY のときだけ付く）
- 全部通れば 0、1 つでも失敗すれば 1。前提不足で実行しなかった job は skip 扱いで失敗にしない
- `--json` で最後に job ごとの成否を出し、`--no-color` で色と上書き描画を切る
- 状態は `.ci/state/<job>.json`、step のログは `.ci/logs/<route>/<step>.log`

## GitHub での見え方

`ci.yml` は job を 3 つしか持たないが、次の情報は失われない。

| 見え方 | 手段 |
| --- | --- |
| job 名 | matrix の `name`（例 `Desktop / Windows`） |
| step の折りたたみ | `report.group()` が `::group::` / `::endgroup::` を出力 |
| 失敗の注釈 | `report.error()` が `::error::` を出力 |
| 実行時間の表 | `report.write_summary()` が `$GITHUB_STEP_SUMMARY` に Markdown 表を書く |
| route の再実行 | matrix の `route.job` 単位で再実行できる |
| 実行 OS | matrix の `route.os` を `run.py --os` に渡す |

Actions UI の step 単位プログレスバーだけは失われる。代替は job サマリの表。

## 検証を追加する

1. `scripts/ci/routes/<route>.py` の `Route.steps` に `Step` を足す
2. 必要なら `Route` の前提（`rust` / `node` / `wasm_bindgen` / `java` / `gradle`）を埋める
3. OS 固有の step なら `routes/platforms/<route>_<os>.py` に `STEPS` を置く（route ファイルは触らない）
4. `python -m scripts.ci.run <route> --step <新しい step 名>` でローカル確認
5. 新しい path を検証対象にするなら `scripts/ci/paths.py` の `PATH_RULES` も更新する

step は shell 文字列ではなく argv で書く。`&&` や `$LASTEXITCODE` に頼ると失敗の検出が抜ける。

## route を追加する

1. `scripts/ci/routes/<new>.py` に `Route` を書く（`runs_on` を必ず埋める）
2. `scripts/ci/routes/__init__.py` の import と `ORDER` に足す
3. `scripts/ci/paths.py` の `PATH_RULES` に、その route を選ぶ path を足す
4. OS ごとに step が違う場合は `scripts/ci/routes/platforms/<route>_<os>.py` を足すだけ（上の 1〜3 は変更不要）

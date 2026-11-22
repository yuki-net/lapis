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

## どこを見て route を選ぶか

`plan` と `watch` は、どの範囲の変更を見るかを 3 つから選ぶ。既定は「PR の差分＋未コミット」。

| モード | コマンド | 見る範囲 |
| --- | --- | --- |
| 既定 | （指定なし） | `base...HEAD`（three-dot）と未コミット（staged/unstaged/untracked）の**和** |
| 未コミットだけ | `--worktree` | 作業ツリーの変更だけ。コミット済みは見ない |
| 全部 | `--full`（`--no-diff` も可） | diff を取らず全 route |

- **three-dot** を使うのは、base 側にしか無いコミットを「削除」と誤認しないため。base が HEAD の祖先なら `base..HEAD` と同じ結果になる
- 未コミットには `git diff HEAD` に加えて `ls-files --others` を含める。新規ディレクトリを足したときに route を取りこぼさない
- `--reason` で、どの範囲を見て何 route 選んだかを出す

```powershell
python -m scripts.ci.plan --selected --reason
# origin/develop...HEAD + worktree: 12 changed path(s) select 3 route(s)
```

- base が解決できない（浅い clone、CI 外）場合は全 route に倒す。**取りこぼすより回しすぎる方を選ぶ**
- CI では作業ツリーが clean なので、既定は実質 PR の差分だけになる

## route 一覧

`python -m scripts.ci.run --list` で確認できる。

| route | `runs_on` | 主な検証 |
| --- | --- | --- |
| `repository` | linux, windows | `scripts/check.py repository architecture`（CI・拡張・開発コマンドの Python テスト、アーキテクチャ検査） |
| `dev_commands` | windows | 開発起動コマンド・`dev-session` の整形とテスト |
| `rust_core` | linux, windows | backend 全体の architecture / format / test / lint と外部拡張 component |
| `host` | windows | Host の architecture / format / test / lint / build と承認済み拡張 PTY・Web 公開 |
| `desktop` | windows | Desktop の architecture / format / test / lint / build と GPUI patch 検証 |
| `mobile` | linux, windows | KMP のユニットテストと Android ビルド（Linux では `./gradlew`、Windows では `gradlew.bat`） |
| `web` | linux, windows | Web のビルドと接続処理テスト（`{npm}` が OS の実行ファイル名になる） |

GitHub の job 名は `name` と `runs_on`（または `target`）から作る（例 `Host / Windows`, `Mobile / Android / Windows`）。

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
- 手元では**手元の OS の step**を実行する。`runs_on` に無い OS でも警告を出して実行する（他 OS のふりはしない）
- `--os` は CI の matrix が渡す。「その OS の variant として実行する」の意味で、ローカルで確かめたいときにも使える
- `{npm}` と `{gradlew}` は対象 OS のコマンド名に展開する（`npm` / `npm.cmd`、`./gradlew` / `gradlew.bat`）
- Windows では `.cmd` / `.bat` を `cmd /c` 経由で起動する（`CreateProcess` が直接実行できないため）
- `{runner_temp}` はローカルでは job ごとに空のディレクトリを作り直す（CI の `RUNNER_TEMP` と同じ前提。前回の残骸で stage が落ちないように）
- 端末が ANSI に対応していない場合は上書き描画をやめ、最後にまとめて出す
- step の上限は `Step.timeout_minutes`、無指定なら `Route.timeout_minutes` から 10 分（後処理のキャッシュ保存分）を引いた値

## まとめて回す（GitHub と同じ並列）

```powershell
python -m scripts.ci.watch                  # PR の差分 + 未コミット
python -m scripts.ci.watch --worktree       # 未コミットの変更だけ
python -m scripts.ci.watch --full           # 全 route
python -m scripts.ci.watch host desktop     # route を指定
python -m scripts.ci.watch --jobs 2         # 同時実行数を絞る
```

route ごとに 1 job（**この機械の OS**）を並列で起動し、1 画面に進捗を出す。名前は GitHub に出る job 名（`Host / Windows`, `Mobile / Android / Windows`）に合わせるが、**手元の OS で実行している以上、その OS 名で出す**（Linux のふりはしない）。

```
CI

OK Host / Windows             [########--] 14/18   12m03s
>> Desktop / Windows          [#####-----]  9/18    7m41s  clients/ui/src/shell/layout.rs
.. Web / Windows              [----------]  0/3      -      (waiting)
```

- 右側は**いま処理しているファイル**。出力が変わると表示も変わるので、止まっていないことの目安になる
- `Ctrl+C` で起動した job を子孫ごと止める（終了コード 130）
- `OK` は緑、`NG` は赤、`>>` は黄、`..` は待機。記号は ASCII なので cp932 の端末でも崩れない（色は TTY のときだけ付く）
- 全部通れば 0、1 つでも失敗すれば 1。前提不足で実行しなかった job は skip 扱いで失敗にしない
- 失敗すると、終了時に**どの job がどの step で何を出して落ちたか**を出す

```
1 passed  1 failed

Host / Windows  Test Host
    error[E0433]: failed to resolve: use of undeclared crate or module `foo`
    logs: .ci/logs/host
```

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

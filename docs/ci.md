# CI の検証

CI で走る検証は、ローカルでも同じものが同じコマンドで走ります。GitHub Actions は最後の確認として使う。

## 何が正本か

| 場所 | 所有するもの |
| --- | --- |
| `scripts/ci/jobs.py` | **唯一の正本**。route 名・OS・タイムアウト・必要な toolchain・step 列 |
| `scripts/ci/paths.py` | 変更 path → route の対応 |
| `scripts/ci/plan.py` | 上 2 つを繋ぎ、実行すべき route を matrix 形式にする |
| `scripts/ci/run.py` | 1 route をローカル / GitHub 共通で実行 |
| `scripts/ci/report.py` | ログ装飾・失敗注釈・実行サマリ |
| `scripts/ci/gate.py` | plan と matrix の結果から CI の成否を決める |
| `.github/workflows/ci.yml` | `plan` / `ci`（matrix）/ `gate` の 3 job のみ。step 列は持たない |

`.github/workflows/ci.yml` に step を書き足さないこと。検証を追加するときは `jobs.py` を編集する（`scripts/ci/test_plan.py` が YAML への直書きを検出して落とす）。

## route 一覧

`python -m scripts.ci.run --list` で確認できる。

| route | CI の runner | 主な検証 |
| --- | --- | --- |
| `repository` | ubuntu-latest | `scripts/check.py repository architecture`（CI・拡張・開発コマンドの Python テスト、アーキテクチャ検査） |
| `dev_windows` | windows-2022 | 開発起動コマンド・`dev-session` の整形とテスト |
| `rust_core` | ubuntu-latest | backend 全体の architecture / format / test / lint と外部拡張 component |
| `host` | windows-2022 | Host の architecture / format / test / lint / build と承認済み拡張 PTY・Web 公開 |
| `desktop` | windows-2022 | Desktop の architecture / format / test / lint / build と GPUI patch 検証 |
| `mobile` | ubuntu-latest | KMP のユニットテストと Android ビルド |
| `web` | ubuntu-latest | Web のビルドと接続処理テスト |

## ローカルで回す

```powershell
python -m scripts.ci.run --list                  # route と前提を確認
python -m scripts.ci.run repository              # route 全体
python -m scripts.ci.run rust_core --step lint   # step を絞る
python -m scripts.ci.run desktop --json          # エージェント向け（ログを抑制）
```

- 成功した step の出力は捨てる。失敗した step だけ末尾 40 行を表示し、全文は `.ci/logs/<route>/<step>.log` に残る
- `--json` は `{route, name, ok, steps:[{name, ok, seconds}]}` だけを返す。ログを読まずに成否を判断できる
- 前提（cargo / node / wasm-bindgen / java / gradle）が無い route は実行せず理由を表示する。`--strict` を付けると失敗扱いになる
- CI の runner OS と違う OS でも、前提が揃えば同じ step を実行する。差はログの装飾とキャッシュだけ

## GitHub での見え方

`ci.yml` は job を 3 つしか持たないが、次の情報は失われない。

| 見え方 | 手段 |
| --- | --- |
| job 名 | matrix の `name`（例 `Desktop / Windows`） |
| step の折りたたみ | `report.group()` が `::group::` / `::endgroup::` を出力 |
| 失敗の注釈 | `report.error()` が `::error::` を出力 |
| 実行時間の表 | `report.write_summary()` が `$GITHUB_STEP_SUMMARY` に Markdown 表を書く |
| route の再実行 | matrix の `route.job` 単位で再実行できる |

Actions UI の step 単位プログレスバーだけは失われる。代替は job サマリの表。

## 検証を追加する

1. `scripts/ci/jobs.py` の該当 `Route.steps` に `Step` を足す
2. 必要なら `Route` の前提（`rust` / `node` / `wasm_bindgen` / `java` / `gradle`）を埋める
3. `python -m scripts.ci.run <route> --step <新しい step 名>` でローカル確認
4. 新しい path を検証対象にするなら `scripts/ci/paths.py` の `PATH_RULES` も更新する

step は shell 文字列ではなく argv で書く。`&&` や `$LASTEXITCODE` に頼ると失敗の検出が抜ける。

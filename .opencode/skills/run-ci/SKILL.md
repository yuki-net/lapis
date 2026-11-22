---
name: run-ci
description: CI と同じ検証をローカルで実行する。変更した領域の検証を選び、失敗箇所だけを読む。コミット前や PR 前に使う。
---

# CI と同じ検証をローカルで回す

GitHub Actions の待ち時間を使わずに、CI と同じ step を同じコマンドで実行する。
route の検証内容は `scripts/ci/routes/<route>.py`、変更 path との対応は
`scripts/ci/paths.py` が正本で、`.github/workflows/ci.yml` は matrix を展開するだけ。

## 手順

1. 何を検証すべきか決める。`python -m scripts.ci.plan --selected --reason` が変更 path から route を選ぶ。既定は PR の差分（three-dot）と未コミットの和。`--worktree` で未コミットだけ、`--full` で全部。
2. route を実行する。

```sh
python -m scripts.ci.run repository
python -m scripts.ci.run desktop --step lint
python -m scripts.ci.run rust_core --json
```

3. 失敗したら、その step のログだけ読む。全文は `.ci/logs/<route>/<step>.log` に落ちている。
4. 直したら同じ route を再実行する。全 route を回す必要はない。

複数 route をまとめて回すときは `python -m scripts.ci.watch` を使う。手元の OS で 1 job/route を並列に起動し、進捗・成否を 1 画面に出す。`Ctrl+C` で起動した job ごと止まる。範囲は `--worktree` / `--full` で切り替える。

所要時間の内訳が要るときは `python -m scripts.ci.measure <route>` を使う（step 別の分数と占有比が出る）。

## 判断の指針

- 成功 step の出力は既定で捨てられる。情報が要るなら `--step` で絞るか、ログファイルを開く
- `--json` は成否だけを返す。まずこれで当たりを付け、失敗した route だけ通常実行する
- 前提（cargo / node / wasm-bindgen / java / gradle）が無い route は実行されず理由が出る。`--strict` で失敗扱いにできる
- CI の runner OS と違っても同じ step を回す。差はログの装飾とキャッシュだけ
- `runs_on` に無い OS の route は門で止まる。`--os` を明示したときだけ実行できる
- 検証を足すときは `scripts/ci/routes/<route>.py` の `Route.steps` に `Step` を足す。`ci.yml` と `jobs.py` は触らない
- OS 固有の step は `runs_on` に OS を足し、`scripts/ci/routes/platforms/<route>_<os>.py` に置く
- path の振り分けを変えるときは `scripts/ci/paths.py` の `PATH_RULES` を編集する

## やってはいけないこと

- `.github/workflows/ci.yml` に step を書き足す（`scripts/ci/test_plan.py` が落とす）
- `--no-prerequisite-check` をローカルで使う（CI 専用。前提の欠落を見落とす）
- テスト条件を緩めて通す

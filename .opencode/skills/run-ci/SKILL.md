---
name: run-ci
description: CI と同じ検証をローカルで実行する。変更した領域の検証を選び、失敗箇所だけを読む。コミット前や PR 前に使う。
---

# CI と同じ検証をローカルで回す

GitHub Actions の待ち時間を使わずに、CI と同じ step を同じコマンドで実行する。
定義は `scripts/ci/jobs.py` が唯一の正本で、`.github/workflows/ci.yml` は matrix を展開するだけ。

## 手順

1. 何を検証すべきか決める。`python -m scripts.ci.plan --selected --base HEAD~1` が変更 path から route を選ぶ。
2. route を実行する。

```sh
python -m scripts.ci.run repository
python -m scripts.ci.run desktop --step lint
python -m scripts.ci.run rust_core --json
```

3. 失敗したら、その step のログだけ読む。全文は `.ci/logs/<route>/<step>.log` に落ちている。
4. 直したら同じ route を再実行する。全 route を回す必要はない。

## 判断の指針

- 成功 step の出力は既定で捨てられる。情報が要るなら `--step` で絞るか、ログファイルを開く
- `--json` は成否だけを返す。まずこれで当たりを付け、失敗した route だけ通常実行する
- 前提（cargo / node / wasm-bindgen / java / gradle）が無い route は実行されず理由が出る。`--strict` で失敗扱いにできる
- CI の runner OS と違っても同じ step を回す。差はログの装飾とキャッシュだけ
- 検証を足すときは `scripts/ci/jobs.py` の `Route.steps` に `Step` を足す。`ci.yml` は触らない

## やってはいけないこと

- `.github/workflows/ci.yml` に step を書き足す（`scripts/ci/test_plan.py` が落とす）
- `--no-prerequisite-check` をローカルで使う（CI 専用。前提の欠落を見落とす）
- テスト条件を緩めて通す

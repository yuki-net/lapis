"""route の実行状態をファイルに書き、`watch.py` が読んで描画する。

`run.py` は 1 route しか知らないため、並列に走らせた複数 route の進捗を
1 画面にまとめるには、route をまたぐ共有の置き場が要る。ここはその
ファイル形式だけを持つ。

有効なのは環境変数 `LAPIS_CI_STATE_DIR` があるときだけ。CI（GitHub）や
通常のローカル実行では何も書かない。
"""

import json
import os
import time
from pathlib import Path

ENV = "LAPIS_CI_STATE_DIR"

# 状態の遷移。`watch.py` の表示と終了コードはこれだけを見る。
PENDING = "pending"
RUNNING = "running"
PASSED = "passed"
FAILED = "failed"
CANCELLED = "cancelled"
SKIPPED = "skipped"

STATUSES = (PENDING, RUNNING, PASSED, FAILED, CANCELLED, SKIPPED)


def directory() -> Path | None:
    """状態の置き場。指定が無ければ None（記録しない）。"""
    value = os.environ.get(ENV)
    return Path(value) if value else None


def path_for(base: Path, job: str) -> Path:
    return base / f"{job}.json"


def write(path: Path, payload: dict) -> None:
    """読む側が壊れた JSON を掴まないよう、一時ファイル経由で置き換える。

    Windows では読み取り中のファイルを置換できず `PermissionError` に
    なることがある（watch が 0.3 秒ごとに読む）。その回は諦め、次の
    flush で書き直す。
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    try:
        os.replace(temporary, path)
    except OSError:
        pass


def read(path: Path) -> dict | None:
    """読めない・書き込み途中・消えた、は「まだ無い」として扱う。"""
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def read_all(base: Path) -> dict[str, dict]:
    """job 名 → 状態。書き込み途中や削除済みは無視する。"""
    states = {}
    for path in sorted(base.glob("*.json")):
        payload = read(path)
        if payload and "job" in payload:
            states[payload["job"]] = payload
    return states


class Writer:
    """1 route の状態を書く。置き場が無ければ全メソッドが何もしない。"""

    def __init__(
        self,
        base: Path | None,
        job: str,
        name: str,
        os_name: str,
        total_steps: int,
    ):
        self._path = path_for(base, job) if base else None
        self._last_pulse = 0.0
        self.data = {
            "job": job,
            "name": name,
            "os": os_name,
            "status": RUNNING,
            "started_at": time.time(),
            "finished_at": None,
            "steps_total": total_steps,
            "steps_done": 0,
            "current_step": None,
            "current_step_started_at": None,
            "last_output_at": None,
            "failed_step": None,
        }
        self.flush()

    def flush(self) -> None:
        if self._path is not None:
            write(self._path, self.data)

    def begin(self, step_name: str) -> None:
        self.data["current_step"] = step_name
        self.data["current_step_started_at"] = time.time()
        self.flush()

    def end(self, step_name: str, ok: bool) -> None:
        self.data["steps_done"] += 1
        if not ok:
            self.data["failed_step"] = step_name
        self.flush()

    def pulse(self) -> None:
        """出力があったことを記録する。毎秒 1 回に間引く。"""
        now = time.time()
        if now - self._last_pulse < 1.0:
            return
        self._last_pulse = now
        self.data["last_output_at"] = now
        self.flush()

    def finish(self, ok: bool) -> None:
        self.data["status"] = PASSED if ok else FAILED
        self.data["finished_at"] = time.time()
        self.data["current_step"] = None
        self.flush()

    def skip(self, reason: str) -> None:
        """前提が無いなどで実行しなかった。失敗ではない。"""
        self.data["status"] = SKIPPED
        self.data["finished_at"] = time.time()
        self.data["current_step"] = None
        self.data["reason"] = reason
        self.flush()

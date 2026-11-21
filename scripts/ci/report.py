"""CI の出力整形。GitHub Actions では注釈とサマリ、ローカルでは表にする。

同じ実行コードから両方を出せるよう、環境変数の有無だけで切り替える。
"""

import json
import os
import sys
import time
from pathlib import Path


def on_github() -> bool:
    return os.environ.get("GITHUB_ACTIONS") == "true"


def group(name: str) -> None:
    if on_github():
        print(f"::group::{name}", flush=True)


def endgroup() -> None:
    if on_github():
        print("::endgroup::", flush=True)


def error(message: str, file: str | None = None, line: int | None = None, column: int | None = None) -> None:
    """失敗を GitHub の注釈とローカルの stderr の両方に出す。"""
    if on_github():
        location = ""
        if file:
            location = f" file={file}"
            if line is not None:
                location += f",line={line}"
                if column is not None:
                    location += f",col={column}"
        print(f"::error{location}::{message}", flush=True)
    else:
        print(f"error: {message}", file=sys.stderr, flush=True)


def warning(message: str) -> None:
    if on_github():
        print(f"::warning::{message}", flush=True)
    else:
        print(f"warning: {message}", file=sys.stderr, flush=True)


def notice(message: str) -> None:
    if on_github():
        print(f"::notice::{message}", flush=True)
    else:
        print(message, flush=True)


def write_summary(text: str) -> None:
    """GitHub の job サマリに Markdown を書く。ローカルでは何もしない。"""
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not path:
        return
    with open(path, "a", encoding="utf-8") as summary:
        summary.write(text.rstrip() + "\n\n")


def duration(seconds: float) -> str:
    if seconds < 60:
        return f"{seconds:.1f}s"
    minutes, rest = divmod(int(seconds), 60)
    return f"{minutes}m{rest:02d}s"


class LocalLog:
    """step ごとのログをファイルへ落とし、失敗時だけ末尾を表示する。

    成功 step の出力は捨てる。エージェントが読む情報量を減らすため。
    """

    def __init__(self, base: Path):
        self.base = base
        self._buffer: list[str] = []

    def _path(self, route: str, step: str) -> Path:
        safe = "".join(c if c.isalnum() or c in "-_." else "-" for c in step)[:80]
        return self.base / route / f"{safe}.log"

    def begin(self, route: str, step: str) -> None:
        self._buffer = []

    def write(self, text: str) -> None:
        self._buffer.append(text)

    def end(self, route: str, step: str, tail_lines: int = 40, show: bool = False) -> Path:
        path = self._path(route, step)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(self._buffer), encoding="utf-8")
        if show:
            lines = "".join(self._buffer).splitlines()
            for line in lines[-tail_lines:]:
                print(f"    {line}", flush=True)
            if len(lines) > tail_lines:
                print(f"    ... ({len(lines) - tail_lines} earlier lines in {path})", flush=True)
        return path


def summary_table(route: str, results: list[dict], extra: dict | None = None) -> str:
    """step 結果の Markdown 表を作る。"""
    lines = [f"### {route}", "", "| step | result | time |", "| --- | --- | --- |"]
    for result in results:
        mark = "skipped" if result.get("skipped") else ("pass" if result["ok"] else "**fail**")
        lines.append(f"| {result['name']} | {mark} | {duration(result['seconds'])} |")
    if extra:
        lines.append("")
        for key, value in extra.items():
            lines.append(f"- {key}: {value}")
    return "\n".join(lines)


def emit_json(payload: dict) -> None:
    print(json.dumps(payload, ensure_ascii=False), flush=True)


def elapsed(start: float) -> float:
    return time.perf_counter() - start

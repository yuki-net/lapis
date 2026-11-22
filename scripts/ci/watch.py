"""複数の route を並列に実行し、進捗を 1 画面で描く。

GitHub Actions の matrix と同じく、route × OS の job を同時に走らせる。
各 job は `run.py` を別プロセスで起動し、進捗は `state.py` のファイルを
読んで描画する（run.py が step の開始・終了で書き換える）。

停止は Ctrl+C。起動した job を子孫ごと止める。

usage:
    python -m scripts.ci.watch                 # 変更 path から選ぶ
    python -m scripts.ci.watch --no-diff       # 全 route
    python -m scripts.ci.watch host desktop    # route を指定
    python -m scripts.ci.watch --jobs 2        # 同時実行数を絞る
"""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

from scripts.ci import plan, state
from scripts.ci.console import configure
from scripts.ci.jobs import ROOT, local_os
from scripts.ci.routes import ORDER, job_name, route_for

GREEN = "\x1b[32m"
RED = "\x1b[31m"
YELLOW = "\x1b[33m"
DIM = "\x1b[2m"
RESET = "\x1b[0m"

# 状態 → (記号, 色)。色が使えないときは記号だけになる。
# 状態 → (記号, 色)。端末のコードページに左右されないよう ASCII だけを使う
# （`✓` は cp932 に無く、Windows の既定コードページで文字化けする）。
MARKS = {
    state.PENDING: ("..", DIM),
    state.RUNNING: (">>", YELLOW),
    state.PASSED: ("OK", GREEN),
    state.FAILED: ("NG", RED),
    state.CANCELLED: ("--", DIM),
    state.SKIPPED: ("--", DIM),
}

BAR_WIDTH = 10

# 出力から「今処理しているファイル」を拾うための拡張子。
FILE_PATTERN = re.compile(
    r"[\w./\\-]+\.(?:rs|py|ts|tsx|js|mjs|kt|kts|java|json|toml|md|lock|yml|yaml)"
)


class Palette:
    def __init__(self, enabled: bool):
        self.enabled = enabled

    def paint(self, text: str, color: str) -> str:
        if not self.enabled or not color:
            return text
        return f"{color}{text}{RESET}"


def duration(seconds: float) -> str:
    if seconds < 60:
        return f"{seconds:.0f}s"
    minutes, rest = divmod(int(seconds), 60)
    return f"{minutes}m{rest:02d}s"


def progress_bar(done: int, total: int) -> str:
    if total <= 0:
        return "-" * BAR_WIDTH
    filled = min(BAR_WIDTH, round(BAR_WIDTH * done / total))
    return "#" * filled + "-" * (BAR_WIDTH - filled)


def status_of(entry: dict, states: dict[str, dict], codes: dict[str, int]) -> dict:
    """表示用の状態。state がまだ無い job は pending として扱う。

    プロセスが終わったのに状態が最終形でない（落ちて書けなかった）場合は、
    実行中のままに見えないよう終了コードで補う。
    """
    found = states.get(entry["job"])
    if found is None:
        # 実行されずに終わった（前提不足で skip した等）場合は終了コードで補う。
        finished = entry["job"] in codes
        return {
            "job": entry["job"],
            "name": entry["name"],
            "status": (state.PASSED if codes[entry["job"]] == 0 else state.FAILED) if finished else state.PENDING,
            "steps_total": entry.get("steps", 0),
            "steps_done": 0,
            "current_step": None,
            "started_at": None,
            "last_output_at": None,
        }
    if found.get("status") not in (state.PASSED, state.FAILED) and entry["job"] in codes:
        found = dict(found)
        found["status"] = state.PASSED if codes[entry["job"]] == 0 else state.FAILED
    return found


def activity_of(found: dict) -> str:
    """いま処理しているファイル（無ければ出力の最終行）。

    出力が変われば表示も変わるので、止まっていないことの目安になる。
    """
    line = found.get("last_line")
    if not line:
        return ""
    match = FILE_PATTERN.search(line)
    if match:
        # パスは省略せずそのまま返す（呼び出し側で幅に切る）。crate 名が
        # 落ちると、どの領域の話か分からなくなる。
        return match.group(0).replace("\\", "/")
    return line[:72]


def line_for(entry: dict, found: dict, now: float, palette: Palette) -> str:
    """1 job の表示行。名前は GitHub の job 名、右は今処理しているファイル。"""
    status = found.get("status", state.PENDING)
    mark, color = MARKS.get(status, ("?", ""))
    total = found.get("steps_total") or entry.get("steps", 0)
    done = found.get("steps_done", 0)
    bar = progress_bar(done, total)

    if found.get("started_at"):
        elapsed = (found.get("finished_at") or now) - found["started_at"]
        elapsed_text = duration(elapsed)
    else:
        elapsed_text = "-"

    if status == state.RUNNING:
        activity = activity_of(found)
    elif status == state.PENDING:
        activity = "(waiting)"
    elif status == state.SKIPPED:
        activity = found.get("reason") or "(skipped)"
    else:
        activity = ""
    activity = activity[:72]

    head = palette.paint(mark, color)
    name = palette.paint(f"{entry['name']:<26}", color if status in (state.FAILED, state.PASSED) else "")
    return f"{head} {name} [{bar}] {done:>2}/{total:<2} {elapsed_text:>7}  {activity}"


def merged_states(entries: list[dict], raw: dict[str, dict], codes: dict[str, int]) -> dict[str, dict]:
    """表示・集計用に、終了コードで補った状態を job 名で引けるようにする。"""
    return {entry["job"]: status_of(entry, raw, codes) for entry in entries}


def failure_lines(entries: list[dict], states: dict[str, dict], palette: Palette) -> list[str]:
    """終了後に「何がどこで落ちたか」を出す。これが無いと原因を追えない。"""
    lines = []
    for entry in entries:
        found = states.get(entry["job"], {})
        if found.get("status") != state.FAILED:
            continue
        step = found.get("failed_step") or "(unknown step)"
        lines.append("")
        lines.append(
            palette.paint(f"{entry['name']}", RED) + palette.paint(f"  {step}", DIM)
        )
        reason = found.get("failure") or found.get("last_line")
        if reason:
            lines.append(f"    {reason[:160]}")
        logs = ROOT / ".ci" / "logs" / entry["job"]
        if logs.exists():
            lines.append(palette.paint(f"    logs: {logs}", DIM))
    return lines


def summary_lines(entries: list[dict], states: dict[str, dict], palette: Palette) -> list[str]:
    passed = [e for e in entries if states.get(e["job"], {}).get("status") == state.PASSED]
    failed = [e for e in entries if states.get(e["job"], {}).get("status") == state.FAILED]
    cancelled = [e for e in entries if states.get(e["job"], {}).get("status") == state.CANCELLED]
    skipped = [e for e in entries if states.get(e["job"], {}).get("status") == state.SKIPPED]
    parts = [
        palette.paint(f"{len(passed)} passed", GREEN),
        palette.paint(f"{len(failed)} failed", RED if failed else DIM),
    ]
    if skipped:
        parts.append(palette.paint(f"{len(skipped)} skipped", DIM))
    if cancelled:
        parts.append(palette.paint(f"{len(cancelled)} cancelled", DIM))
    return ["  ".join(parts)]


def exit_code(entries: list[dict], states: dict[str, dict]) -> int:
    """成功（skip を含む）以外が 1 つでもあれば失敗。"""
    done = (state.PASSED, state.SKIPPED)
    return 0 if all(states.get(e["job"], {}).get("status") in done for e in entries) else 1


class Screen:
    """ターミナルなら同じ行数を上書きし、そうでなければ最後だけ出す。"""

    def __init__(self, stream, cursor: bool):
        self.stream = stream
        self.cursor = cursor and stream.isatty()
        self.drawn = 0

    def draw(self, lines: list[str], final: bool = False) -> None:
        if not self.cursor:
            if final:
                for line in lines:
                    print(line, flush=True)
            return
        if self.drawn:
            self.stream.write(f"\x1b[{self.drawn}A")
        for line in lines:
            self.stream.write("\x1b[2K" + line + "\n")
        self.stream.flush()
        self.drawn = len(lines)

    def close(self) -> None:
        if self.cursor and self.drawn:
            self.stream.write("\n")
            self.stream.flush()


def terminate(process: subprocess.Popen) -> None:
    """job の子孫ごと止める。run.py がさらに子プロセスを持つため木ごと。"""
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(process.pid)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    else:
        try:
            os.killpg(os.getpgid(process.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            process.kill()


def spawn(job: str, state_dir: Path) -> subprocess.Popen:
    """job を子プロセスで起動する。OS は子プロセスが自分で決める。"""
    environment = dict(os.environ)
    environment[state.ENV] = str(state_dir)
    return subprocess.Popen(
        [sys.executable, "-m", "scripts.ci.run", job, "--quiet"],
        cwd=ROOT,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=(os.name != "nt"),
        creationflags=(subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0),
    )


def run(entries: list[dict], jobs: int, colors: bool) -> int:
    state_dir = ROOT / ".ci" / "state"
    shutil.rmtree(state_dir, ignore_errors=True)
    state_dir.mkdir(parents=True, exist_ok=True)

    palette = Palette(colors)
    screen = Screen(sys.stdout, cursor=colors)
    queue = list(entries)
    running: dict[str, subprocess.Popen] = {}
    codes: dict[str, int] = {}
    started = time.time()

    def render(final: bool = False) -> None:
        now = time.time()
        raw = state.read_all(state_dir)
        states = merged_states(entries, raw, codes) if final else raw
        lines = ["CI", ""]
        for entry in entries:
            lines.append(line_for(entry, status_of(entry, raw, codes), now, palette))
        lines.append("")
        if final:
            lines.extend(summary_lines(entries, states, palette))
            lines.extend(failure_lines(entries, states, palette))
        else:
            lines.append(
                palette.paint(
                    f"{len(codes)}/{len(entries)} finished / {duration(now - started)} / Ctrl+C to stop",
                    DIM,
                )
            )
        screen.draw(lines, final=final)

    try:
        while queue or running:
            while queue and len(running) < jobs:
                entry = queue.pop(0)
                running[entry["job"]] = spawn(entry["job"], state_dir)
            for job, process in list(running.items()):
                code = process.poll()
                if code is not None:
                    codes[job] = code
                    del running[job]
            render()
            time.sleep(0.3)
    except KeyboardInterrupt:
        for process in running.values():
            terminate(process)
        for process in running.values():
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
        render()
        screen.close()
        print(palette.paint("cancelled", YELLOW), flush=True)
        return 130
    except BaseException:
        # watch 自身が落ちても job を孤児にしない。
        for process in running.values():
            terminate(process)
        raise

    render(final=True)
    screen.close()
    return exit_code(entries, state.read_all(state_dir))


def local_entries(keys: list[str]) -> list[dict]:
    """手元の OS で走らせる job の一覧。

    CI の matrix は「どの OS で走るか」を決めるが、手元は 1 台しかない。
    他 OS のふりはせず、この機械の OS の step を実行して、その OS 名で出す。
    """
    os_name = local_os()
    entries = []
    for key in keys:
        route = route_for(key)
        entries.append(
            {
                "job": route.key,
                "name": job_name(route, os_name),
                "os": os_name,
                "steps": len(route.steps) + len(route.platform_steps.get(os_name, ())),
            }
        )
    return entries


def main() -> int:
    configure()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("route", nargs="*", help="実行する route（省略時は変更 path から選ぶ）")
    parser.add_argument("--base", default=plan.DEFAULT_BASE, help="diff の基準 ref")
    parser.add_argument("--no-diff", action="store_true", help="diff を取らずに全 route を対象にする")
    parser.add_argument("--jobs", type=int, default=0, help="同時実行数（0 は全 job 並列）")
    parser.add_argument("--json", action="store_true", help="最後に結果を JSON で出す")
    parser.add_argument("--no-color", action="store_true", help="色と上書き描画を使わない")
    args = parser.parse_args()

    if args.route:
        keys = [key for key in ORDER if key in set(args.route)]
        unknown = sorted(set(args.route) - set(keys))
        if unknown:
            print(f"unknown route: {', '.join(unknown)}", file=sys.stderr)
            return 2
    else:
        paths = None if args.no_diff else plan.changed_paths(args.base)
        keys = plan.route_keys(paths)

    entries = local_entries(keys)
    if not entries:
        print("no job selected", file=sys.stderr)
        return 0

    jobs = args.jobs if args.jobs > 0 else len(entries)
    # パイプやリダイレクト先に ANSI を流さない。
    colors = (
        not args.no_color
        and os.environ.get("NO_COLOR") is None
        and sys.stdout.isatty()
    )
    code = run(entries, jobs, colors=colors)

    if args.json:
        states = state.read_all(ROOT / ".ci" / "state")
        print(json.dumps({
            "ok": code == 0,
            "jobs": [
                {
                    "job": entry["job"],
                    "os": entry["os"],
                    "status": states.get(entry["job"], {}).get("status", state.PENDING),
                }
                for entry in entries
            ],
        }, ensure_ascii=False))
    return code


if __name__ == "__main__":
    sys.exit(main())

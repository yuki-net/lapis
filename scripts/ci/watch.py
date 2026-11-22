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

from scripts.ci import console, plan, state
from scripts.ci.console import configure
from scripts.ci.model import ROOT
from scripts.ci.routes import ORDER

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


ANSI_PATTERN = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")


def visible_length(text: str) -> int:
    return len(ANSI_PATTERN.sub("", text))


def truncate_ansi(text: str, width: int) -> str:
    """表示幅で切る。色のエスケープは幅に数えず、途中で切らない。"""
    if visible_length(text) <= width:
        return text
    out: list[str] = []
    visible = 0
    index = 0
    while index < len(text):
        match = ANSI_PATTERN.match(text, index)
        if match:
            out.append(match.group(0))
            index = match.end()
            continue
        if visible >= width:
            break
        out.append(text[index])
        visible += 1
        index += 1
    return "".join(out) + RESET


# 代替画面バッファ。全画面を毎フレーム描き直すので、行数を数えてカーソルを
# 戻す必要がない（折り返しやスクロールで数え間違えると古い行が残る）。
# vim / less / htop と同じ仕組みで、Windows 10 以降・macOS・Linux の
# 主要な端末が対応している。
ENTER_ALT_SCREEN = "\x1b[?1049h"
LEAVE_ALT_SCREEN = "\x1b[?1049l"
HIDE_CURSOR = "\x1b[?25l"
SHOW_CURSOR = "\x1b[?25h"


def fit_lines(lines: list[str], width: int, height: int) -> list[str]:
    """画面に収める。

    - 幅で切る: 1 行が折り返すと画面の行数と合わなくなるため必ず切る
    - 高さで切る: はみ出すと代替画面内でスクロールし、上端が欠けるため
    """
    fitted = [truncate_ansi(line, width) for line in lines]
    if height <= 0 or len(fitted) <= height:
        return fitted
    omitted = len(fitted) - height + 1
    return fitted[: height - 1] + [f"... {omitted} more line(s)"]


class Screen:
    """端末なら全画面を描き直し、パイプなら確定行だけを追記する。"""

    def __init__(self, stream, ansi: bool):
        self.stream = stream
        self.ansi = ansi
        self.active = False

    def frame(self, lines: list[str]) -> None:
        """1 フレーム描く。ANSI が無ければ何もしない。"""
        if not self.ansi:
            return
        if not self.active:
            self.stream.write(ENTER_ALT_SCREEN + HIDE_CURSOR)
            self.active = True
        size = shutil.get_terminal_size((80, 24))
        fitted = fit_lines(lines, size.columns - 1, size.lines - 1)
        self.stream.write("\x1b[H")
        for line in fitted:
            self.stream.write("\x1b[2K" + line + "\n")
        # 前のフレームで余った行を消す。
        self.stream.write("\x1b[J")
        self.stream.flush()

    def commit(self, line: str) -> None:
        """ANSI が使えないときの確定行。使えるときは frame に含める。"""
        if self.active:
            return
        self.stream.write(line + "\n")
        self.stream.flush()

    def close(self, final_lines: list[str]) -> None:
        """代替画面を抜け、最終結果を通常の画面に残す。"""
        if self.active:
            self.stream.write(SHOW_CURSOR + LEAVE_ALT_SCREEN)
            self.stream.flush()
            self.active = False
        for line in final_lines:
            self.stream.write(line + "\n")
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


def commit_line(entry: dict, found: dict, index: int, total: int, palette: Palette) -> str:
    """終わった job の 1 行。後からスクロールして読み返せる形にする。"""
    status = found.get("status", state.PENDING)
    mark, color = MARKS.get(status, ("?", ""))
    steps = found.get("steps_total") or entry.get("steps", 0)
    if found.get("started_at") and found.get("finished_at"):
        elapsed = duration(found["finished_at"] - found["started_at"])
    else:
        elapsed = "-"
    detail = ""
    if status == state.FAILED and found.get("failed_step"):
        detail = f"  {found['failed_step']}"
    elif status == state.SKIPPED and found.get("reason"):
        detail = f"  {found['reason']}"
    head = palette.paint(mark, color)
    name = palette.paint(entry["name"], color if status in (state.FAILED, state.PASSED) else "")
    return f"[{index}/{total}] {head} {name}  {found.get('steps_done', 0)}/{steps}  {elapsed}{detail}"


def run(entries: list[dict], jobs: int, colors: bool, scope: str = "") -> int:
    state_dir = ROOT / ".ci" / "state"
    shutil.rmtree(state_dir, ignore_errors=True)
    state_dir.mkdir(parents=True, exist_ok=True)

    palette = Palette(colors)
    screen = Screen(sys.stdout, ansi=colors)
    if scope and not colors:
        # ANSI が無い端末では全画面を描けないので、見出しだけ 1 行残す。
        screen.commit(palette.paint(f"CI  {scope}", DIM))
    queue = list(entries)
    running: dict[str, subprocess.Popen] = {}
    codes: dict[str, int] = {}
    committed: set[str] = set()
    started = time.time()

    def final_lines() -> list[str]:
        raw = state.read_all(state_dir)
        states = merged_states(entries, raw, codes)
        lines = summary_lines(entries, states, palette)
        lines.extend(failure_lines(entries, states, palette))
        return lines

    def frame_lines(raw: dict[str, dict]) -> list[str]:
        """全 job を 1 画面に収めたフレーム。"""
        now = time.time()
        lines = [palette.paint(f"CI  {scope}", DIM) if scope else "CI", ""]
        for entry in entries:
            lines.append(line_for(entry, status_of(entry, raw, codes), now, palette))
        lines.append("")
        lines.append(
            palette.paint(
                f"[{len(codes)}/{len(entries)} done] {duration(now - started)}"
                "  Ctrl+C to stop",
                DIM,
            )
        )
        return lines

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

            raw = state.read_all(state_dir)
            if colors:
                screen.frame(frame_lines(raw))
            else:
                # 全画面が使えない端末では、終わった job だけ確定行で出す。
                for entry in entries:
                    job = entry["job"]
                    if job in codes and job not in committed:
                        committed.add(job)
                        screen.commit(
                            commit_line(
                                entry,
                                status_of(entry, raw, codes),
                                len(committed),
                                len(entries),
                                palette,
                            )
                        )
            time.sleep(0.3)
    except KeyboardInterrupt:
        for process in running.values():
            terminate(process)
        for process in running.values():
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
        screen.close(final_lines() + [palette.paint("cancelled", YELLOW)])
        return 130
    except BaseException:
        # watch 自身が落ちても job を孤児にしない。
        screen.close([])
        for process in running.values():
            terminate(process)
        raise

    screen.close(final_lines())
    return exit_code(entries, state.read_all(state_dir))


def main() -> int:
    configure()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("route", nargs="*", help="実行する route（省略時は変更 path から選ぶ）")
    parser.add_argument("--base", default=plan.DEFAULT_BASE, help="diff の基準 ref")
    parser.add_argument(
        "--full",
        "--no-diff",
        dest="full",
        action="store_true",
        help="差分を取らずに全 route を対象にする",
    )
    parser.add_argument(
        "--worktree",
        action="store_true",
        help="コミット済みの差分を無視し、未コミットの変更だけを対象にする",
    )
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
        scope = "route 指定"
    else:
        scope, paths = plan.select_paths(args)
        keys = plan.route_keys(paths)

    entries = plan.local_entries(keys)
    if not entries:
        print(f"no job selected ({scope})", file=sys.stderr)
        return 0

    jobs = args.jobs if args.jobs > 0 else len(entries)
    # ANSI が使えないコンソール（VT 無効・パイプ）では上書き描画をやめ、
    # 最後にまとめて出す。中途半端に有効化すると行が増え続ける。
    colors = (
        not args.no_color
        and os.environ.get("NO_COLOR") is None
        and console.supports_ansi()
    )
    code = run(entries, jobs, colors=colors, scope=scope)

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

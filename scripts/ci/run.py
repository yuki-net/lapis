"""1 route の検証を、ローカルと GitHub で同じコードから実行する。

GitHub では `.github/workflows/ci.yml` がこの script だけを呼ぶ。
ローカルでは同じ script を直接叩く。差分は出力の装飾と、前提
（toolchain・node・wasm-bindgen 等）の確認方法だけ。
"""

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from scripts.ci import report, state
from scripts.ci.console import configure
from scripts.ci.jobs import ROOT, ORDER, local_os, requirements, route_for
from scripts.ci.model import RUNNERS


def configure_output() -> None:
    """日本語を含むログを Windows の既定コードページでも落とさない。

    CI の Windows runner は cp1252 で、日本語の step ログを print すると
    UnicodeEncodeError で step ごと落ちる。
    """
    configure()

# 前提を判定するためのコマンド。値を返せば充足。
PREREQUISITES = {
    "python": ("python", "--version"),
    "cargo": ("cargo", "--version"),
    "node": ("node", "--version"),
    "npm": ("npm", "--version"),
    "wasm-bindgen": ("wasm-bindgen", "--version"),
    "java": ("java", "-version"),
    "gradle": ("gradle", "--version"),
}

# job の後処理（target/ のキャッシュ保存）に使う時間。step の既定上限は
# route の上限からこれを引いて確保する。
POST_JOB_RESERVE_MINUTES = 10


def runner_temp(override: Path | None = None) -> Path:
    """step の一時領域。

    GitHub は job ごとに空の `RUNNER_TEMP` が渡される。ローカルで同じ場所を
    使うと、前回の出力が残って「入力が空ではない」で落ちる（拡張の stage が
    これに当たる）。呼び出し側が実行ごとのディレクトリを渡す。
    """
    if override is not None:
        return override
    value = os.environ.get("RUNNER_TEMP")
    if value:
        return Path(value)
    return Path(tempfile.gettempdir())


def substitute(value: str, os_name: str | None = None, temp: Path | None = None) -> str:
    """step の引数にある実行環境依存のプレースホルダを解決する。

    `{root}` はリポジトリルートの絶対パス。テストバイナリの作業ディレクトリは
    cargo が crate ディレクトリにするため、リポジトリ相対で解決させたい値は
    絶対パスに展開しておく。

    `{npm}` と `{gradlew}` は対象 OS のコマンド名に展開する。OS で名前が
    変わるものだけをここに集め、route 側は 1 つの step 列で済ませる。
    """
    os_name = os_name or local_os()
    exe = ".exe" if os.name == "nt" else ""
    npm = "npm.cmd" if os_name == "windows" else "npm"
    gradlew = "gradlew.bat" if os_name == "windows" else "./gradlew"
    return (
        value.replace("{root}", str(ROOT))
        .replace("{runner_temp}", str(runner_temp(temp)))
        .replace("{exe}", exe)
        .replace("{npm}", npm)
        .replace("{gradlew}", gradlew)
    )


def host_command(command: list[str]) -> list[str]:
    """この機械で実行できる形に直す。

    Windows の `CreateProcess` は `.cmd` / `.bat` を直接実行できない
    （`npm.cmd` や `gradlew.bat` がこれに当たる）。`cmd /c` を挟む。
    """
    if os.name == "nt" and command and command[0].lower().endswith((".cmd", ".bat")):
        return ["cmd", "/c", *command]
    return command


def missing_prerequisites(route) -> list[str]:
    """ローカルで route に必要な前提のうち、満たさないものを返す。"""
    required = list(requirements(route))
    if route.node and "npm" not in required:
        required.append("npm")
    missing = []
    for name in required:
        if name == "gradle":
            exists = shutil.which("gradle") is not None or (ROOT / "clients/mobile/gradlew").exists()
        elif name == "posix":
            exists = os.name != "nt"
        else:
            command = PREREQUISITES.get(name)
            exists = command is not None and shutil.which(command[0]) is not None
        if not exists:
            missing.append(name)
    return missing


def run_step(route, step, log: report.LocalLog | None, writer=None, os_name: str | None = None, temp: Path | None = None) -> dict:
    """1 step を実行し、結果を返す。失敗は例外にせず結果に落とす。

    出力はパイプではなく実ファイルへ書かせ、待ちながら追記分を表示する。

    - パイプだと、cargo やテストが起動した孫プロセスが書き込み端を掴んだまま
      になり、直接の子が終了しても EOF を待ち続ける（CI で 18 分ハングした）
    - 終了までバッファすると、ハングしたときに何も分からない（同じ step が
      2 回 25 分止まって原因不明だった）

    追記読み出しなので、長い step でも進行が見え、ハング箇所も特定できる。
    step に個別の上限が無い場合は route の上限から後処理（キャッシュ保存）分を
    差し引いた値を使い、job 予算を食い潰さない。
    """
    directory = ROOT / (route.working_directory or "") / (step.directory or "")
    command = [substitute(part, os_name, temp) for part in step.command]
    environment = dict(os.environ)
    environment.update({key: substitute(value, os_name, temp) for key, value in step.env.items()})
    limit = (step.timeout_minutes or max(1, route.timeout_minutes - POST_JOB_RESERVE_MINUTES)) * 60
    if log is not None:
        log.begin(route.key, step.name)

    attempts = step.retries + 1
    started = time.perf_counter()
    output = ""
    ok = False
    for attempt in range(1, attempts + 1):
        report.group(step.name)
        started = time.perf_counter()
        retryable = True
        try:
            output, ok = execute_step(command, directory, environment, limit, writer)
        except StepStartError as error:
            # 実行ファイルが無い等は待っても変わらないので再試行しない。
            output, ok, retryable = str(error), False, False
        report.endgroup()

        if log is not None:
            log.write(output)

        if ok or attempt == attempts or not retryable:
            if log is not None:
                log.end(route.key, step.name, show=not ok and not report.on_github())
            break
        report.warning(f"{step.name} failed (attempt {attempt}/{attempts}); retrying in {step.retry_delay_seconds}s")
        time.sleep(step.retry_delay_seconds)

    return {
        "name": step.name,
        "ok": ok,
        "seconds": report.elapsed(started),
        "output": output,
    }


class StepStartError(RuntimeError):
    """step のコマンドを起動できなかった（実行ファイルが無い等）。"""


def execute_step(
    command: list[str],
    directory: Path,
    environment: dict[str, str],
    limit: float | None,
    writer=None,
) -> tuple[str, bool]:
    """コマンドを実行し、(出力, 成功したか) を返す。

    出力は一時ファイルへ書き、待っている間に追記分を stdout へ流す。
    タイムアウト時は子孫プロセスも含めて止める。
    """
    capture = tempfile.NamedTemporaryFile(suffix=".log", delete=False)
    try:
        try:
            process = subprocess.Popen(
                host_command(command),
                cwd=directory,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=capture,
                stderr=subprocess.STDOUT,
                # タイムアウト時に子孫まで止められるよう、独立したプロセス
                # グループ / セッションで起動する。
                start_new_session=(os.name != "nt"),
                creationflags=(subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0),
            )
        except OSError as error:
            # 実行ファイルが無い（別 OS 用の ./gradlew など）。route 全体を
            # 落とさず、その step の失敗として扱う。
            raise StepStartError(f"{type(error).__name__}: {error}") from error
        capture.close()

        deadline = (time.perf_counter() + limit) if limit else None
        timed_out = False
        position = 0
        while True:
            try:
                process.wait(timeout=0.5)
                break
            except subprocess.TimeoutExpired:
                pass
            position, chunk = pump_output(capture.name, position)
            if writer is not None:
                writer.observe(chunk)
                writer.pulse()
            if deadline is not None and time.perf_counter() > deadline:
                terminate_tree(process)
                timed_out = True
                break

        position, _ = pump_output(capture.name, position)
        output = decode_output(Path(capture.name).read_bytes())
        if timed_out:
            output += f"\nstep exceeded {limit / 60:g} minutes\n"
            report.error("step timed out; terminated the process tree")
            return output, False
        return output, process.returncode == 0
    finally:
        capture.close()
        try:
            Path(capture.name).unlink(missing_ok=True)
        except PermissionError:
            # 孫プロセスがまだ掴んでいる場合は消せない。OS の一時領域に任せる。
            pass


def decode_output(data: bytes) -> str:
    """step の出力を復号する。

    cargo などは UTF-8、Windows の OS メッセージは cp932 で出る。UTF-8 で
    読めなければ OS のコードページで読み直す（日本語の失敗理由が化けるのを
    防ぐ）。
    """
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        pass
    for encoding in ("mbcs", "cp1252"):
        try:
            return data.decode(encoding)
        except (UnicodeDecodeError, LookupError):
            continue
    return data.decode("utf-8", errors="replace")


def pump_output(path: str, position: int) -> tuple[int, str]:
    """出力ファイルの position 以降を読み、(新しい位置, 読んだ分) を返す。"""
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            handle.seek(position)
            chunk = handle.read()
    except FileNotFoundError:
        return position, ""
    if chunk and report.on_github():
        # step の進行が CI ログから見えるようにする。ハング箇所の特定に要る。
        for line in chunk.splitlines():
            print(line, flush=True)
    return position + len(chunk), chunk


def terminate_tree(process: subprocess.Popen) -> None:
    """タイムアウトした step の子孫プロセスも含めて止める。"""
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
    try:
        process.wait(timeout=30)
    except subprocess.TimeoutExpired:
        process.kill()


def tail(output: str, lines: int = 40) -> str:
    parts = output.splitlines()
    return "\n".join(parts[-lines:])


# 失敗理由として拾う行の手がかり。上から順に優先する。
ERROR_HINTS = (
    "error[E",
    "error:",
    "ERROR:",
    "npm ERR",
    "FAILED",
    "failed",
    "panic",
    "assertion",
    "Traceback",
    "FileNotFoundError",
    "WinError",
    "not recognized",
    "No such file",
)


def error_excerpt(output: str, lines: int = 2) -> str:
    """失敗理由になる行だけを抜き出す。

    末尾を丸ごと出すと watch の一覧が潰れるので、エラーらしい行を優先し、
    無ければ最後の数行に落とす。
    """
    stripped = [line.strip() for line in output.splitlines() if line.strip()]
    if not stripped:
        return ""
    hints = tuple(hint.lower() for hint in ERROR_HINTS)
    picked = [line for line in stripped if any(hint in line.lower() for hint in hints)]
    chosen = picked[-lines:] if picked else stripped[-lines:]
    return " / ".join(chosen)[:300]


def execute(
    route,
    only: list[str] | None,
    quiet: bool,
    os_name: str | None = None,
    temp: Path | None = None,
) -> dict:
    """route の step を実行する。`os_name` の差分 step も後ろに足す。"""
    os_name = os_name or local_os()
    all_steps = [*route.steps, *route.platform_steps.get(os_name, ())]
    steps = all_steps
    if only:
        wanted = {name.lower() for name in only}
        steps = [step for step in all_steps if step.name.lower() in wanted]
        if not steps:
            available = ", ".join(step.name for step in all_steps) or "none"
            raise SystemExit(f"no step matched {only}\navailable: {available}")

    log = None if report.on_github() else report.LocalLog(ROOT / ".ci" / "logs")
    writer = state.Writer(state.directory(), route.key, route.name, os_name, len(steps))
    # ローカルでは job ごとの一時領域を毎回作り直す。CI の RUNNER_TEMP と
    # 同じ前提（空であること）を守るため。
    work_temp = temp
    created_temp = False
    if work_temp is None and not report.on_github():
        work_temp = Path(tempfile.mkdtemp(prefix=f"lapis-ci-{route.key}-"))
        created_temp = True
    results = []
    try:
        for step in steps:
            if step.requires and missing_prerequisites_for_step(step):
                report.notice(f"skip {step.name} (requires {', '.join(step.requires)})")
                results.append({"name": step.name, "ok": True, "skipped": True, "seconds": 0.0, "output": ""})
                continue
            report.notice(f"run {step.name}: {' '.join(step.command)}")
            writer.begin(step.name)
            result = run_step(route, step, log, writer, os_name, work_temp)
            writer.end(step.name, result["ok"])
            if not result["ok"]:
                writer.note_failure(error_excerpt(result["output"]))
                report.error(f"{route.name}: {step.name} failed")
                if not quiet and not report.on_github():
                    print(tail(result["output"]), file=sys.stderr, flush=True)
                results.append(result)
                break
            results.append(result)

        ok = all(result["ok"] for result in results)
        writer.finish(ok)
    except KeyboardInterrupt:
        # 中断（Ctrl+C など）でも、watch 側が「実行中」のまま残らないようにする。
        writer.data["status"] = state.CANCELLED
        writer.flush()
        raise
    except BaseException as error:
        # 想定外の例外は失敗として残す（理由が消えると追えない）。
        writer.data["status"] = state.FAILED
        writer.data["failure"] = f"{type(error).__name__}: {error}"[:300]
        writer.flush()
        raise
    finally:
        if created_temp and work_temp is not None:
            shutil.rmtree(work_temp, ignore_errors=True)
    payload = {
        "route": route.key,
        "name": route.name,
        "ok": ok,
        "steps": [{key: value for key, value in result.items() if key != "output"} for result in results],
    }
    log_path = ROOT / ".ci" / "logs" / route.key
    if log_path.exists():
        payload["logs"] = str(log_path)

    if not report.on_github():
        print()
        print(report.summary_table(route.name, results))
    report.write_summary(report.summary_table(route.name, results))
    return payload


def missing_prerequisites_for_step(step) -> list[str]:
    missing = []
    for name in step.requires:
        if name == "posix":
            if os.name == "nt":
                missing.append(name)
        else:
            command = PREREQUISITES.get(name)
            if command is None or shutil.which(command[0]) is None:
                missing.append(name)
    return missing


def list_routes() -> None:
    print(f"{'route':12} {'name':20} {'runs_on':20} {'platform':16} {'matches this os':16} requires")
    for key in ORDER:
        route = route_for(key)
        runs_on = ",".join(route.runs_on)
        platform = ",".join(sorted(route.platform_steps)) or "-"
        matches = "yes" if local_os() in route.runs_on else "no (same steps)"
        print(
            f"{key:12} {route.name:20} {runs_on:20} {platform:16} {matches:16} "
            f"{', '.join(requirements(route)) or '-'}"
        )
    print()
    print(
        "usage: python -m scripts.ci.run <route> "
        "[--list] [--step <name>] [--os <os>] [--json]"
    )


def main() -> int:
    configure_output()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("route", nargs="?", help="実行する route（--list で一覧）")
    parser.add_argument("--list", action="store_true", help="route 一覧と前提を表示する")
    parser.add_argument("--step", action="append", help="指定した step だけ実行する（複数可）")
    parser.add_argument(
        "--os",
        choices=tuple(RUNNERS),
        help="この OS として実行する。CI の matrix から渡す。ローカルで別 OS の検証を試すときにも使う",
    )
    parser.add_argument("--json", action="store_true", help="結果を JSON で stdout に出す")
    parser.add_argument("--quiet", action="store_true", help="失敗 step の抜粋を出さない")
    parser.add_argument("--strict", action="store_true", help="実行できない route を失敗として扱う")
    parser.add_argument("--no-prerequisite-check", action="store_true", help="前提確認を省略する（CI 用）")
    args = parser.parse_args()

    if args.list or not args.route:
        list_routes()
        return 0 if args.list else 2

    route = route_for(args.route)

    # 手元では手元の OS で、その OS 用の step を実行する（他 OS のふりは
    # しない）。CI の matrix が別 OS しか含まない場合は、その旨を伝える。
    target_os = args.os or local_os()
    if not report.on_github() and args.os is None and local_os() not in route.runs_on:
        runs_on = ", ".join(route.runs_on)
        report.warning(
            f"{route.key} runs on {runs_on} in CI; running the {local_os()} steps here"
        )

    if not report.on_github() and not args.no_prerequisite_check:
        missing = missing_prerequisites(route)
        if missing:
            message = f"{route.key} needs {', '.join(missing)} on this machine"
            report.warning(message)
            if args.strict:
                report.error(message)
                return 1
            # watch から起動されている場合に「失敗」に見えないよう記録する。
            state.Writer(
                state.directory(), route.key, route.name, args.os or local_os(), 0
            ).skip(f"missing {', '.join(missing)}")
            print(json.dumps({"route": route.key, "ok": True, "skipped": True, "reason": f"missing {missing}"}))
            return 0

    payload = execute(route, args.step, args.quiet, target_os)
    if args.json:
        report.emit_json(payload)
    return 0 if payload["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
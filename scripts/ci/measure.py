"""route の step 別所要時間を測って並べる。

CI の 38 分がどの step に食われているかを、ローカルの実測で確認するための
道具。`run.py` の `--json` は step ごとの `seconds` を返すので、それを
読みやすい表にするだけ。CI の job サマリと同じ情報をコマンドから出せる。

usage:
    python -m scripts.ci.measure host
    python -m scripts.ci.measure host --step "Test Host" --step "Build terminal component"
    python -m scripts.ci.measure desktop --output .ci/measure/desktop.json
"""

import argparse
import json
import sys
import time
from pathlib import Path

from scripts.ci import run
from scripts.ci.console import configure
from scripts.ci.model import ROOT
from scripts.ci.routes import route_for


def table(route_name: str, steps: list[dict], total: float) -> str:
    """step 別の所要時間を、合計と占有比つきで Markdown 表にする。"""
    lines = [f"### {route_name}  {total / 60:.1f}m", "", "| # | step | time | share |", "| --- | --- | --- | --- |"]
    for index, step in enumerate(steps, 1):
        seconds = step.get("seconds", 0.0)
        share = (seconds / total * 100) if total else 0.0
        mark = "skipped" if step.get("skipped") else ("" if step["ok"] else " **fail**")
        lines.append(f"| {index} | {step['name']}{mark} | {seconds / 60:.1f}m | {share:.0f}% |")
    return "\n".join(lines)


def main() -> int:
    configure()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("route", help="計測する route（python -m scripts.ci.run --list）")
    parser.add_argument("--step", action="append", help="計測する step を絞る（複数可）")
    parser.add_argument("--json", action="store_true", help="結果を JSON で出す")
    parser.add_argument("--output", help="結果を JSON ファイルへ保存する")
    parser.add_argument("--strict", action="store_true", help="前提が欠けた route を失敗として扱う")
    args = parser.parse_args()

    route = route_for(args.route)
    missing = run.missing_prerequisites(route)
    if missing and not args.strict:
        print(f"{route.key} needs {', '.join(missing)} on this machine", file=sys.stderr)
        print(json.dumps({"route": route.key, "ok": True, "skipped": True, "reason": f"missing {missing}"}))
        return 0
    if missing:
        print(f"{route.key} needs {', '.join(missing)} on this machine", file=sys.stderr)
        return 1

    started = time.perf_counter()
    payload = run.execute(route, args.step, quiet=True)
    total = time.perf_counter() - started
    payload["total_seconds"] = round(total, 1)

    text = table(route.name, payload["steps"], total)
    if args.json:
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print(text)
    if args.output:
        path = Path(args.output)
        if not path.is_absolute():
            path = ROOT / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
        if not args.json:
            print(f"\nwrote {path}")

    # 遅い step を上から 5 件。どの step を削るべきかの判断材料。
    if not args.json and len(payload["steps"]) > 3:
        slowest = sorted(payload["steps"], key=lambda step: -step.get("seconds", 0.0))[:5]
        print("\nslowest:")
        for step in slowest:
            print(f"  {step['seconds'] / 60:6.1f}m  {step['name']}")
    return 0 if payload["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())

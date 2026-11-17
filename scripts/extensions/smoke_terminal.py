"""Terminal component の生データと PTY 操作を模擬 Host で確認する。"""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import queue
import subprocess
import threading

ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "target/terminal-component"
COMPONENT = TARGET / "wasm32-wasip2/debug/lapis_terminal_component.wasm"
WORKER = ROOT / "target/debug" / ("lapis-extension-host.exe" if os.name == "nt" else "lapis-extension-host")


def main() -> None:
    subprocess.run(["cargo", "build", "--locked", "--manifest-path", "extensions/terminal/component/Cargo.toml",
                    "--target", "wasm32-wasip2", "--target-dir", str(TARGET)], cwd=ROOT, check=True)
    child = subprocess.Popen([str(WORKER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True,
                             creationflags=0x08000000 if os.name == "nt" else 0)
    lines = queue.Queue()
    def reader():
        for line in child.stdout:
            lines.put(line)
    threading.Thread(target=reader, daemon=True).start()
    calls = []
    def send(value):
        child.stdin.write(json.dumps(value) + "\n")
        child.stdin.flush()
    def receive():
        try:
            return json.loads(lines.get(timeout=15))
        except queue.Empty as error:
            raise RuntimeError("拡張ホストが応答しません") from error
    def request(value):
        send(value)
        while True:
            response = receive()
            if response.get("kind") != "call":
                return response
            assert response["extension"] == "lapis.terminal", response
            method = response["method"]
            calls.append(method)
            if method == "pty.v1.spawn":
                result = "pty-1"
            elif method == "pty.v1.read":
                result = {"bytes": [65, 66], "next": 2, "lost_before": None,
                          "exited": None, "read_error": None, "write_error": None}
            else:
                assert method in {"pty.v1.attach", "pty.v1.write", "pty.v1.resize",
                                  "pty.v1.kill", "pty.v1.detach", "pty.v1.forget"}, method
                result = None
            send({"kind": "reply", "id": response["id"], "result": {"Ok": result}})
    def dispatch(operation, payload):
        return request({"command": "dispatch", "id": "lapis.terminal",
                        "operation": "terminal.v1." + operation, "payload": json.dumps(payload)})
    try:
        response = request({"command": "activate", "id": "lapis.terminal",
                            "component": base64.b64encode(COMPONENT.read_bytes()).decode("ascii"),
                            "require_dispatch": True})
        assert response["ok"], response
        assert not request({"command": "dispatch", "id": "lapis.terminal",
                            "operation": "terminal.v2.list", "payload": "{}"})["ok"]
        assert json.loads(dispatch("list", {})["result"]) == {"sessions": []}
        result = dispatch("start", {"program": "shell", "working_directory": ".",
                                    "columns": 80, "rows": 24})
        assert result["ok"] and json.loads(result["result"]) == {"id": "pty-1"}, result
        assert json.loads(dispatch("list", {})["result"]) == {
            "sessions": [{"id": "pty-1", "columns": 80, "rows": 24}]
        }
        assert dispatch("write", {"id": "pty-1", "bytes": [65]})["ok"]
        assert dispatch("resize", {"id": "pty-1", "columns": 100, "rows": 30})["ok"]
        assert json.loads(dispatch("list", {})["result"])["sessions"][0]["columns"] == 100
        output = dispatch("read", {"id": "pty-1", "offset": 0, "max_bytes": 8, "timeout_ms": 0})
        assert output["ok"] and json.loads(output["result"])["bytes"] == [65, 66], output
        assert not dispatch("read", {"id": "pty-1", "offset": 0, "max_bytes": 100000, "timeout_ms": 0})["ok"]
        assert dispatch("close", {"id": "pty-1"})["ok"]
        assert json.loads(dispatch("list", {})["result"]) == {"sessions": []}
        assert not dispatch("write", {"id": "pty-1", "bytes": [65]})["ok"]
        assert calls == ["pty.v1.spawn", "pty.v1.attach", "pty.v1.write", "pty.v1.resize",
                         "pty.v1.read", "pty.v1.kill", "pty.v1.detach", "pty.v1.forget"], calls
        assert request({"command": "shutdown"})["ok"]
        assert child.wait(timeout=15) == 0
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        child.stdin.close()
        child.stdout.close()
        child.stderr.close()


if __name__ == "__main__":
    main()

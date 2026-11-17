"""Terminal component の生データと PTY 操作を模擬 Host で確認する。"""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "target/terminal-component"
COMPONENT = TARGET / "wasm32-wasip2/debug/lapis_terminal_component.wasm"
WORKER = ROOT / "target/debug" / ("lapis-extension-host.exe" if os.name == "nt" else "lapis-extension-host")


def main() -> None:
    # root Cargo.toml を持たないコピーを使い、別 repository への移動可能性を検査する。
    (ROOT / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="terminal-standalone-", dir=ROOT / "target") as temp:
        standalone = Path(temp)
        shutil.copytree(ROOT / "extensions/terminal/api", standalone / "api")
        shutil.copytree(ROOT / "extensions/terminal/component", standalone / "component")
        subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(standalone / "component/Cargo.toml"),
                        "--target", "wasm32-wasip2", "--target-dir", str(TARGET)], cwd=ROOT, check=True)
        # 画面モデルも本体の workspace 設定に依存せず、拡張と一緒に持ち出せる。
        model = standalone / "model"
        model.mkdir()
        for name in ("contract", "portable-renderer"):
            shutil.copytree(ROOT / "extensions/terminal" / name, model / name)
        (model / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\nmembers = ["contract", "portable-renderer"]\n', encoding="utf-8"
        )
        shutil.copyfile(ROOT / "Cargo.lock", model / "Cargo.lock")
        model_check = ["cargo", "check", "--manifest-path", str(model / "Cargo.toml"),
                       "--package", "terminal-portable-renderer", "--target", "wasm32-wasip2",
                       "--target-dir", str(TARGET)]
        subprocess.run(model_check, cwd=ROOT, check=True)
        subprocess.run([*model_check, "--locked"], cwd=ROOT, check=True)
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
            if method.startswith("pty.v1.") and method != "pty.v1.spawn":
                assert response["payload"]["id"] == "pty-1", response
            if method == "pty.v1.spawn":
                result = "pty-1"
            elif method == "process.info.v1.get":
                result = {"pty": "conpty", "default_shell": {"program": "powershell.exe", "arguments": ["-NoLogo"]}}
            elif method == "pty.v1.read":
                result = {"bytes": [65, 66], "next": 2, "lost_before": None,
                          "exited": None, "read_error": None, "write_error": None}
            else:
                assert method in {"pty.v1.attach", "pty.v1.write", "pty.v1.resize",
                                  "pty.v1.kill", "pty.v1.detach", "pty.v1.forget"}, method
                result = None
            send({"kind": "reply", "id": response["id"], "result": {"Ok": result}})
    def dispatch(operation, payload, session="session-1", workspace="workspace-1"):
        return request({"command": "dispatch", "id": "lapis.terminal",
                        "operation": "terminal.v1." + operation, "payload": json.dumps(payload),
                        "context": {"session": session, "workspace": workspace}})
    try:
        response = request({"command": "activate", "id": "lapis.terminal",
                            "component": base64.b64encode(COMPONENT.read_bytes()).decode("ascii"),
                            "require_dispatch": True})
        assert response["ok"], response
        assert not request({"command": "dispatch", "id": "lapis.terminal",
                            "operation": "terminal.v2.list", "payload": "{}",
                            "context": {"session":"session-1", "workspace":"workspace-1"}})["ok"]
        info = dispatch("info", {})
        assert info["ok"] and json.loads(info["result"]) == {
            "pty": "conpty", "default_shell": {"program": "powershell.exe", "arguments": ["-NoLogo"]}
        }, info
        assert json.loads(dispatch("list", {})["result"]) == {"sessions": []}
        assert not dispatch("start", {"program": "shell", "working_directory": "x" * 9000,
                                      "columns": 80, "rows": 24})["ok"]
        result = dispatch("start", {"program": "shell", "working_directory": ".",
                                    "columns": 80, "rows": 24})
        assert result["ok"] and json.loads(result["result"]) == {"id": "pty-1", "owner": "session-1"}, result
        assert json.loads(dispatch("list", {})["result"]) == {
            "sessions": [{"id": "pty-1", "columns": 80, "rows": 24, "working_directory": ".", "owner": "session-1"}]
        }
        assert json.loads(dispatch("list", {}, workspace="workspace-2")["result"]) == {"sessions": []}
        assert not dispatch("read", {"id":"pty-1", "offset":0, "max_bytes":8, "timeout_ms":0}, workspace="workspace-2")["ok"]
        assert json.loads(dispatch("attach", {"id":"pty-1"}, session="session-2")["result"])["owner"] == "session-1"
        assert json.loads(dispatch("claim", {"id":"pty-1"}, session="session-2")["result"])["owner"] == "session-2"
        assert not dispatch("write", {"id":"pty-1", "bytes":[65]})["ok"]
        assert not dispatch("release", {"id":"pty-1"})["ok"]
        assert json.loads(dispatch("claim", {"id":"pty-1"})["result"])["owner"] == "session-1"
        assert json.loads(dispatch("detach", {"id":"pty-1"}, session="session-2")["result"])["owner"] == "session-1"
        assert json.loads(dispatch("detach", {"id":"pty-1"})["result"])["owner"] is None
        assert json.loads(dispatch("claim", {"id":"pty-1"})["result"])["owner"] == "session-1"
        notice = {"command": "dispatch", "id": "lapis.terminal", "payload": "{}",
                  "operation": "lapis.system.session-detached",
                  "context": {"session": "session-2", "workspace": "workspace-1"}}
        assert request(notice)["ok"]
        assert json.loads(dispatch("list", {})["result"])["sessions"][0]["owner"] == "session-1"
        notice["context"]["session"] = "session-1"
        assert request(notice)["ok"]
        assert json.loads(dispatch("list", {})["result"])["sessions"][0]["owner"] is None
        assert json.loads(dispatch("claim", {"id":"pty-1"})["result"])["owner"] == "session-1"
        notice["operation"] = "lapis.system.release-claims"
        assert request(notice)["ok"]
        assert json.loads(dispatch("list", {})["result"])["sessions"][0]["owner"] is None
        assert json.loads(dispatch("claim", {"id":"pty-1"})["result"])["owner"] == "session-1"
        assert dispatch("write", {"id": "pty-1", "bytes": [65]})["ok"]
        assert dispatch("resize", {"id": "pty-1", "columns": 100, "rows": 30})["ok"]
        assert json.loads(dispatch("list", {})["result"])["sessions"][0]["columns"] == 100
        output = dispatch("read", {"id": "pty-1", "offset": 0, "max_bytes": 8, "timeout_ms": 0})
        assert output["ok"] and json.loads(output["result"])["bytes"] == [65, 66], output
        assert not dispatch("read", {"id": "pty-1", "offset": 0, "max_bytes": 100000, "timeout_ms": 0})["ok"]
        assert dispatch("close", {"id": "pty-1"})["ok"]
        assert json.loads(dispatch("list", {})["result"]) == {"sessions": []}
        assert not dispatch("write", {"id": "pty-1", "bytes": [65]})["ok"]
        assert calls == ["process.info.v1.get", "pty.v1.spawn", "pty.v1.attach", "pty.v1.write", "pty.v1.resize",
                         "pty.v1.read", "pty.v1.kill", "pty.v1.detach", "pty.v1.forget"], calls
        stable = "client-" + "a" * 32
        start = {"requested_id": stable, "program": "shell", "working_directory": ".", "columns": 80, "rows": 24}
        assert json.loads(dispatch("start", start)["result"])["id"] == stable
        before = len(calls)
        assert not dispatch("start", start)["ok"]
        assert not dispatch("start", start, workspace="workspace-2")["ok"]
        assert len(calls) == before
        assert json.loads(dispatch("list", {})["result"])["sessions"][0]["id"] == stable
        assert not dispatch("write", {"id": stable, "bytes": [65]}, workspace="workspace-2")["ok"]
        assert dispatch("write", {"id": stable, "bytes": [65]})["ok"]
        assert dispatch("close", {"id": stable})["ok"]
        start["requested_id"] = "../unsafe"
        assert not dispatch("start", start)["ok"]
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

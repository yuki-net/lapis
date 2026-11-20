"""実際の component の PTY WIT と双方向 IPC を、模擬 Host で検証する。"""
from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import queue
import subprocess
import threading


ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "target/pty-extension"
COMPONENT = TARGET / "wasm32-wasip2/debug/example_pty_extension.wasm"
WORKER = ROOT / "target/debug" / ("lapis-extension-host.exe" if os.name == "nt" else "lapis-extension-host")


def exchange(denied: bool) -> None:
    child = subprocess.Popen([str(WORKER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             text=True, creationflags=0x08000000 if os.name == "nt" else 0)
    output = queue.Queue()
    def read_lines():
        for line in child.stdout:
            output.put(line)
    threading.Thread(target=read_lines, daemon=True).start()
    def send(value):
        child.stdin.write(json.dumps(value) + "\n")
        child.stdin.flush()
    def receive():
        try:
            return json.loads(output.get(timeout=15))
        except queue.Empty as error:
            raise RuntimeError("拡張ホストが応答しません") from error
    try:
        send({"command": "activate", "id": "dev.example.pty", "component": base64.b64encode(COMPONENT.read_bytes()).decode("ascii"), "require_dispatch": True})
        assert receive()["ok"]
        send({"command": "dispatch", "id": "dev.example.pty", "operation": "example.spawn", "payload": "{}",
              "context": {"session": "session-1", "workspace": "workspace-1"}})
        call = receive()
        assert call["kind"] == "call" and call["extension"] == "dev.example.pty" and call["method"] == "pty.v1.spawn", call
        result = {"Err": "not approved"} if denied else {"Ok": "pty-1"}
        send({"kind": "reply", "id": call["id"], "result": result})
        if not denied:
            attach = receive()
            assert attach["kind"] == "call" and attach["id"] != call["id"] and attach["extension"] == call["extension"] and attach["method"] == "pty.v1.attach", attach
            assert attach["payload"] == {"id": "pty-1"}
            send({"kind": "reply", "id": attach["id"], "result": {"Ok": None}})
        assert receive()["ok"]
        send({"command": "shutdown"})
        deactivate = receive()
        assert deactivate["kind"] == "call" and deactivate["extension"] == call["extension"] and deactivate["method"] == "pty.v1.attach", deactivate
        send({"kind": "reply", "id": deactivate["id"], "result": {"Err": "not approved"} if denied else {"Ok": None}})
        assert receive()["ok"]
        assert child.wait(timeout=15) == 0
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        child.stdin.close()
        child.stdout.close()
        child.stderr.close()


def main() -> None:
    subprocess.run(["cargo", "build", "--locked", "-p", "extension-runtime", "--bin", "lapis-extension-host"], cwd=ROOT, check=True)
    base = ["cargo", "build", "--locked", "--manifest-path", "examples/pty-extension/Cargo.toml", "--target", "wasm32-wasip2", "--target-dir", str(TARGET)]
    subprocess.run(base, cwd=ROOT, check=True)
    exchange(False)
    subprocess.run(["cargo", "run", "--locked", "-p", "extension-runtime", "--example", "pty_ipc", "--", str(WORKER), str(COMPONENT), "allowed"], cwd=ROOT, check=True)
    subprocess.run(base + ["--features", "expect-denied"], cwd=ROOT, check=True)
    exchange(True)
    subprocess.run(["cargo", "run", "--locked", "-p", "extension-runtime", "--example", "pty_ipc", "--", str(WORKER), str(COMPONENT), "denied"], cwd=ROOT, check=True)
    print("component の PTY WIT 呼び出しと拒否を模擬 Host で確認しました")


if __name__ == "__main__":
    main()

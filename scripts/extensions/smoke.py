"""独立 workspace の component を拡張ホストで起動し、操作して停止する。"""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import subprocess


def main() -> None:
    root = Path(__file__).resolve().parents[2]
    component = root / "target/external-extension/wasm32-wasip2/debug/example_lapis_extension.wasm"
    worker = root / "target/debug" / ("lapis-extension-host.exe" if os.name == "nt" else "lapis-extension-host")
    requests = [
        {"command": "handshake"},
        {"command": "activate", "id": "dev.example.hello", "component": base64.b64encode(component.read_bytes()).decode("ascii"), "require_dispatch": True},
        {"command": "dispatch", "id": "dev.example.hello", "operation": "echo", "payload": "hello"},
        {"command": "deactivate", "id": "dev.example.hello"},
        {"command": "shutdown"},
    ]
    payload = "".join(json.dumps(request) + "\n" for request in requests)
    result = subprocess.run(
        [str(worker)], input=payload, text=True, capture_output=True, check=True, timeout=30,
        creationflags=0x08000000 if os.name == "nt" else 0,
    )
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    if len(responses) != len(requests) or any(not response.get("ok") for response in responses):
        raise RuntimeError(f"拡張ホストの応答が不正です: {responses}")
    if responses[0].get("lifecycle_version") != "1.0.0":
        raise RuntimeError("ライフサイクル契約の版が一致しません")
    if responses[0].get("dispatch_version") != "1.0.0" or responses[2].get("result") != "hello":
        raise RuntimeError("拡張操作契約の版または応答が一致しません")
    print("外部 workspace の component を起動・操作・停止しました")


if __name__ == "__main__":
    main()

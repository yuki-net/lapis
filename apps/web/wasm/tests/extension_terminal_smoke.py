"""隔離した Host と Bridge で Web の拡張 Terminal を検証する。"""

import json
import base64
import os
from pathlib import Path
import queue
import re
import select
import socket
import subprocess
import tempfile
import threading
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT))
from scripts.release import ed25519
FLAGS = getattr(subprocess, "CREATE_NO_WINDOW", 0)
SUFFIX = ".exe" if os.name == "nt" else ""


def read_line(stream):
    lines = queue.Queue()
    threading.Thread(target=lambda: lines.put(stream.readline()), daemon=True).start()
    try:
        return lines.get(timeout=30)
    except queue.Empty as error:
        raise RuntimeError("process did not respond") from error


def proxy(downstream, bridge_port):
    upstream = socket.create_connection(("127.0.0.1", bridge_port), timeout=10)
    try:
        header = b""
        while b"\r\n\r\n" not in header:
            chunk = downstream.recv(4096)
            if not chunk or len(header) + len(chunk) > 16384:
                return
            header += chunk
        headers, tail = header.split(b"\r\n\r\n", 1)
        lines = [line for line in headers.split(b"\r\n") if not line.lower().startswith(b"origin:")]
        upstream.sendall(b"\r\n".join(lines) + b"\r\nOrigin: http://localhost:5173\r\n\r\n" + tail)
        downstream.settimeout(None)
        upstream.settimeout(None)
        while True:
            ready, _, _ = select.select([downstream, upstream], [], [], 30)
            if not ready:
                break
            for source in ready:
                data = source.recv(65536)
                if not data:
                    return
                (upstream if source is downstream else downstream).sendall(data)
    except (ConnectionError, OSError):
        # クライアントの正常終了でも Windows は reset を返すことがある。
        pass
    finally:
        downstream.close()
        upstream.close()


def start_host(root, state, certificate):
    args = [
        str(ROOT / f"target/debug/lapis-host{SUFFIX}"), "--workspace", str(root),
        "--certificate", str(certificate), "--listen", "127.0.0.1:0",
        "--state-directory", str(state),
    ]
    if certificate.exists():
        args.append("--replace-certificate")
    process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, encoding="utf-8", errors="replace", creationflags=FLAGS)
    try:
        for _ in range(10):
            line = read_line(process.stdout).strip()
            match = re.fullmatch(r"address=(\S+) server_name=(\S+) workspace=(\S+)", line)
            if match:
                read_line(process.stdout)  # command help
                return process, match.groups()
        raise RuntimeError("Host did not start")
    except Exception:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=15)
        raise


def stop_host(process):
    if process and process.poll() is None:
        process.stdin.write("quit\n")
        process.stdin.flush()
        process.wait(timeout=15)


def main():
    if os.name != "nt":
        raise RuntimeError("native Terminal smoke test requires Windows")
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        workspace, state = directory / "workspace", directory / "state"
        source, bindings = directory / "staged", directory / "bindings"
        workspace.mkdir()
        # テスト専用の鍵を隔離された Host にだけ登録する。
        seed = bytes(range(32))
        seed_path = directory / "test-seed"
        seed_path.write_bytes(seed)
        trusted = state / "extensions" / "trusted-keys.json"
        trusted.parent.mkdir(parents=True)
        trusted.write_text(json.dumps({"schema": 1, "keys": {
            "web-smoke": base64.b64encode(ed25519.public_key(seed)).decode("ascii")
        }}), encoding="utf-8")
        subprocess.run(["python", "-m", "scripts.extensions.stage", "extensions", str(source),
                        "--target-dir", "target", "--debug"], cwd=ROOT, check=True, creationflags=FLAGS)
        package, certificate = directory / "terminal.lapis", directory / "host.der"
        subprocess.run(["python", "-m", "scripts.extensions.package", str(source / "terminal"), str(package),
                        "--seed-file", str(seed_path), "--key-id", "web-smoke"],
                       cwd=ROOT, check=True, creationflags=FLAGS)
        subprocess.run(["wasm-bindgen", "--target", "nodejs", "--out-dir", str(bindings),
                        str(ROOT / "target/wasm32-unknown-unknown/web/web_terminal_smoke.wasm")], check=True, creationflags=FLAGS)
        host = bridge = None
        try:
            host, _ = start_host(workspace, state, certificate)
            host.stdin.write(f'ext-install "{package}"\n')
            host.stdin.flush()
            if "installed=lapis.terminal" not in read_line(host.stdout):
                raise RuntimeError("Terminal package installation failed")
            stop_host(host)
            host, (address, server_name, workspace_id) = start_host(workspace, state, certificate)
            host.stdin.write("pair\n")
            host.stdin.flush()
            match = re.fullmatch(r"Pairing token \(1 hour, one use\): ([0-9a-f]{64})", read_line(host.stdout).strip())
            if not match:
                raise RuntimeError("pairing failed")
            port = socket.socket()
            port.bind(("127.0.0.1", 0))
            bridge_port = port.getsockname()[1]
            port.close()
            bridge = subprocess.Popen([
                str(ROOT / f"target/debug/lapis-web-bridge{SUFFIX}"), "--listen", f"127.0.0.1:{bridge_port}",
                "--host", address, "--server-name", server_name, "--certificate", str(certificate),
                "--origin", "http://localhost:5173",
            ], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace", creationflags=FLAGS)
            if "listening on" not in read_line(bridge.stderr):
                raise RuntimeError("Bridge did not start")
            listener = socket.socket()
            listener.bind(("127.0.0.1", 0))
            listener.listen(2)
            proxy_port = listener.getsockname()[1]
            def accept():
                # 最初の接続と、同じ credential による再接続。
                for _ in range(2):
                    downstream, _ = listener.accept()
                    threading.Thread(target=proxy, args=(downstream, bridge_port), daemon=True).start()
                listener.close()
            threading.Thread(target=accept, daemon=True).start()
            script = ("const fs=require('fs'), api=require(process.argv[1]);"
                      "const x=JSON.parse(fs.readFileSync(0,'utf8'));"
                      "api.extension_terminal_smoke(x.url,x.token,x.workspace)"
                      ".then(x=>{if(x!=='ok')throw Error('bad result');console.log('Web extension terminal: OK');process.exit(0)})"
                      ".catch(e=>{console.error(String(e));process.exit(1)})")
            data = json.dumps({"url": f"ws://127.0.0.1:{proxy_port}/remote", "token": match.group(1), "workspace": workspace_id})
            result = subprocess.run(["node", "-e", script, str(bindings / "web_terminal_smoke.js")], input=data,
                                    text=True, encoding="utf-8", errors="replace", capture_output=True,
                                    timeout=80, creationflags=FLAGS)
            if result.returncode:
                raise RuntimeError(f"Web client failed: {result.stderr}")
            print(result.stdout.strip())
        finally:
            if bridge and bridge.poll() is None:
                bridge.terminate()
                bridge.wait(timeout=15)
            stop_host(host)


if __name__ == "__main__":
    main()

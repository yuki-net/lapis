import json, os, re, select, socket, subprocess, tempfile, threading
from pathlib import Path

root = Path(__file__).resolve().parents[4]
flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
suffix = '.exe' if os.name == 'nt' else ''

def proxy(downstream, bridge_port):
    upstream = socket.create_connection(('127.0.0.1', bridge_port), timeout=10)
    try:
        header = b''
        while b'\r\n\r\n' not in header:
            chunk = downstream.recv(4096)
            if not chunk or len(header) + len(chunk) > 16384:
                return
            header += chunk
        headers, tail = header.split(b'\r\n\r\n', 1)
        lines = [line for line in headers.split(b'\r\n') if not line.lower().startswith(b'origin:')]
        upstream.sendall(b'\r\n'.join(lines) + b'\r\nOrigin: http://localhost:5173\r\n\r\n' + tail)
        downstream.settimeout(None)
        upstream.settimeout(None)
        while True:
            ready, _, _ = select.select([downstream, upstream], [], [], 10)
            if not ready: break
            for source in ready:
                data = source.recv(65536)
                if not data: return
                (upstream if source is downstream else downstream).sendall(data)
    finally:
        downstream.close()
        upstream.close()

def accept_clients(listener, bridge_port):
    try:
        for _ in range(4):
            downstream, _ = listener.accept()
            threading.Thread(target=proxy, args=(downstream, bridge_port), daemon=True).start()
    finally:
        listener.close()

with tempfile.TemporaryDirectory() as directory:
    cert = Path(directory) / 'host.der'
    bindings = Path(directory) / 'bindings'
    subprocess.run(['wasm-bindgen', '--target', 'nodejs', '--out-dir', str(bindings), str(root / 'target/wasm32-unknown-unknown/web/lapis_web.wasm')], check=True, creationflags=flags)
    host = subprocess.Popen([str(root / f'target/debug/lapis-host{suffix}'), '--workspace', directory, '--certificate', str(cert), '--listen', '127.0.0.1:0'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, creationflags=flags)
    bridge = None
    try:
        match = re.fullmatch(r'address=(\S+) server_name=(\S+) workspace=(\S+)', host.stdout.readline().strip())
        if not match: raise RuntimeError('host startup failed')
        address, server_name, workspace = match.groups()
        host.stdout.readline()
        other = Path(directory) / 'other workspace'
        other.mkdir()
        host.stdin.write(f'register {other}\n'); host.stdin.flush()
        match = re.fullmatch(r'workspace=(\S+)', host.stdout.readline().strip())
        if not match: raise RuntimeError('workspace registration failed')
        other_workspace = match.group(1)
        host.stdin.write('pair-user\n'); host.stdin.flush()
        match = re.fullmatch(r'User session pairing token \(5 minutes, one use\): ([0-9a-f]{64})', host.stdout.readline().strip())
        if not match: raise RuntimeError('pairing failed')
        token = match.group(1)
        port_socket = socket.socket()
        port_socket.bind(('127.0.0.1', 0))
        bridge_port = port_socket.getsockname()[1]
        port_socket.close()
        bridge = subprocess.Popen([str(root / f'target/debug/lapis-web-bridge{suffix}'), '--listen', f'127.0.0.1:{bridge_port}', '--host', address, '--server-name', server_name, '--certificate', str(cert), '--origin', 'http://localhost:5173'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, creationflags=flags)
        if 'listening on' not in bridge.stderr.readline(): raise RuntimeError('bridge startup failed')
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0)); listener.listen(1)
        proxy_port = listener.getsockname()[1]
        thread = threading.Thread(target=accept_clients, args=(listener, bridge_port), daemon=True)
        thread.start()
        data = json.dumps({'url': f'ws://127.0.0.1:{proxy_port}/remote', 'token': token, 'workspace': workspace, 'other_workspace': other_workspace})
        script = "const fs=require('fs'); const api=require(process.argv[1]); const x=JSON.parse(fs.readFileSync(0,'utf8')); api.bridge_smoke(x.url,x.token,x.workspace,x.other_workspace).then(v=>{if(v!=='ok') throw Error('unexpected result'); console.log('Wasm workspace switch, workbench, document conflicts and reconnect: OK')}).catch(e=>{console.error(String(e));process.exitCode=1})"
        result = subprocess.run(['node', '-e', script, str(bindings / 'lapis_web.js')], input=data, text=True, capture_output=True, timeout=20, creationflags=flags)
        if result.returncode: raise RuntimeError('Wasm client failed: ' + result.stderr)
        print(result.stdout.strip())
        thread.join(timeout=5)
    finally:
        if bridge:
            bridge.terminate(); bridge.wait(timeout=10)
        if host.poll() is None:
            host.stdin.write('quit\n'); host.stdin.flush()
        host.wait(timeout=10)

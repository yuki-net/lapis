"""ビルド済み native Terminal を ABI v1 経由で操作する。"""

import ctypes
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
NAME = "lapis_terminal_native.dll" if os.name == "nt" else (
    "liblapis_terminal_native.dylib" if sys.platform == "darwin" else "liblapis_terminal_native.so"
)


class View(ctypes.Structure):
    _fields_ = [("ptr", ctypes.c_void_p), ("length", ctypes.c_size_t)]


class Buffer(ctypes.Structure):
    _fields_ = [("ptr", ctypes.c_void_p), ("length", ctypes.c_size_t)]


HostCall = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p, View, View, ctypes.POINTER(Buffer))
HostRelease = ctypes.CFUNCTYPE(None, ctypes.c_void_p, Buffer)


class HostApi(ctypes.Structure):
    _fields_ = [("version", ctypes.c_uint32), ("size", ctypes.c_uint32),
                ("context", ctypes.c_void_p), ("call", HostCall), ("release", HostRelease)]


Activate = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.POINTER(HostApi), ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(Buffer))
Dispatch = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p, View, View, View, View, ctypes.POINTER(Buffer))
Deactivate = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(Buffer))
Release = ctypes.CFUNCTYPE(None, Buffer)


class ExtensionApi(ctypes.Structure):
    _fields_ = [("version", ctypes.c_uint32), ("size", ctypes.c_uint32),
                ("activate", Activate), ("dispatch", Dispatch), ("deactivate", Deactivate), ("release", Release)]


def view(data):
    data = data.encode("utf-8")
    buffer = ctypes.create_string_buffer(data)
    return View(ctypes.addressof(buffer), len(data)), buffer


def build_standalone():
    target = ROOT / "target" / "native-terminal-standalone"
    with tempfile.TemporaryDirectory(prefix="terminal-native-", dir=ROOT / "target") as temp:
        root = Path(temp)
        for relative in ("protocol/extension-api/native", "extensions/terminal/api",
                         "extensions/terminal/host", "extensions/terminal/native"):
            shutil.copytree(ROOT / relative, root / relative)
        (root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\nmembers = ["protocol/extension-api/native", '
            '"extensions/terminal/api", "extensions/terminal/host", "extensions/terminal/native"]\n',
            encoding="utf-8",
        )
        command = ["cargo", "build", "--offline", "--manifest-path", str(root / "Cargo.toml"),
                   "--target-dir", str(target), "--package", "lapis-terminal-native"]
        subprocess.run(command, check=True)
        subprocess.run([*command, "--locked"], check=True)
    return target / "debug" / NAME


def main():
    library = ctypes.CDLL(str(build_standalone()))
    entry = library.lapis_extension_v1
    entry.restype = ctypes.POINTER(ExtensionApi)
    api = entry().contents
    assert api.version == 1 and api.size >= ctypes.sizeof(ExtensionApi)
    buffers = {}
    calls = []

    @HostCall
    def call(_context, method, payload, output):
        method = ctypes.string_at(method.ptr, method.length).decode("utf-8")
        request = json.loads(ctypes.string_at(payload.ptr, payload.length))
        calls.append(method)
        if method == "process.info.v1.get":
            result = {"pty": "conpty", "default_shell": {"program": "shell", "arguments": []}}
        elif method == "pty.v1.spawn":
            assert request["cwd"] == "."
            result = "pty-1"
        elif method == "pty.v1.read":
            result = {"bytes": [111, 107], "next": 2, "lost_before": None,
                      "exited": None, "read_error": None, "write_error": None}
        elif method in {"pty.v1.attach", "pty.v1.kill", "pty.v1.detach", "pty.v1.forget"}:
            result = None
        else:
            raise AssertionError(method)
        data = json.dumps(result).encode("utf-8")
        buffer = ctypes.create_string_buffer(data)
        address = ctypes.addressof(buffer)
        buffers[address] = buffer
        output[0] = Buffer(address, len(data))
        return 0

    @HostRelease
    def host_release(_context, value):
        buffers.pop(value.ptr, None)

    host = HostApi(1, ctypes.sizeof(HostApi), None, call, host_release)
    instance = ctypes.c_void_p()
    output = Buffer()
    assert api.activate(ctypes.byref(host), ctypes.byref(instance), ctypes.byref(output)) == 0
    api.release(output)

    def dispatch(operation, payload, workspace="workspace"):
        values = [view(operation), view(json.dumps(payload)), view("client"), view(workspace)]
        output = Buffer()
        status = api.dispatch(instance, *(item[0] for item in values), ctypes.byref(output))
        response = ctypes.string_at(output.ptr, output.length).decode("utf-8") if output.length else ""
        api.release(output)
        return status, response

    status, response = dispatch("terminal.v1.start-profile", {"profile": None, "working_directory": ".", "columns": 80, "rows": 24})
    assert status == 0
    identifier = json.loads(response)["id"]
    status, response = dispatch("terminal.v1.read", {"id": identifier, "offset": 0, "max_bytes": 8, "timeout_ms": 0})
    assert status == 0 and json.loads(response)["bytes"] == [111, 107]
    assert dispatch("terminal.v1.attach", {"id": identifier}, "other")[0] != 0
    output = Buffer()
    assert api.deactivate(instance, ctypes.byref(output)) == 0
    api.release(output)
    assert calls == ["process.info.v1.get", "pty.v1.spawn", "pty.v1.attach", "pty.v1.read", "pty.v1.kill", "pty.v1.detach"]
    assert not buffers
    print("native terminal ABI v1 OK")


if __name__ == "__main__":
    main()

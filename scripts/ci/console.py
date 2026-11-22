"""CI 用 script の出力エンコーディングと端末の制御を揃える。

Windows の CI runner は既定コードページが cp1252 のため、日本語を含む
ログを print すると UnicodeEncodeError で step ごと落ちる。どの script から
呼ばれても UTF-8 + 置換で書き出せるようにする。

ローカルの Windows では逆に、書き出す側を UTF-8 にしてもコンソールが
cp932 のままだと日本語が化ける。コンソールの出力コードページも UTF-8 に
揃える。併せて ANSI エスケープ（画面の上書き描画）を有効にする。

どちらもコンソール全体で共有される設定なので、終了時に元へ戻す。
"""

import os
import sys

# 8000 番以降で「ANSI を使えるか」を watch に伝える。
_ansi_available = False


def configure() -> None:
    """stdout / stderr を UTF-8 に切り替える。失敗しても致命的ではない。"""
    global _ansi_available
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):
            pass

    _ansi_available = _stream_is_tty(sys.stdout)
    if os.name == "nt" and _ansi_available:
        _install_console_settings()


def supports_ansi() -> bool:
    """この出力先で ANSI エスケープ（上書き描画）が使えるか。

    configure() の後に呼ぶ。TTY でない（パイプ・リダイレクト）なら False。
    """
    return _ansi_available


def _stream_is_tty(stream) -> bool:
    try:
        return bool(stream.isatty())
    except (AttributeError, ValueError):
        return False


def _install_console_settings() -> None:
    """コンソールを UTF-8 + ANSI 有効にし、終了時に元へ戻す。"""
    try:
        import atexit
        import ctypes

        kernel32 = ctypes.windll.kernel32
        previous: list[tuple] = []

        previous_cp = kernel32.GetConsoleOutputCP()
        if previous_cp and previous_cp != 65001:
            if kernel32.SetConsoleOutputCP(65001):
                previous.append(("cp", previous_cp))

        # ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004。これが無いと
        # `\x1b[2K` などが解釈されず、進捗画面が行ごとに増えていく。
        handle = kernel32.GetStdHandle(-11)  # STD_OUTPUT_HANDLE
        mode = ctypes.c_uint32()
        if handle and kernel32.GetConsoleMode(handle, ctypes.byref(mode)):
            if not mode.value & 0x0004:
                if kernel32.SetConsoleMode(handle, mode.value | 0x0004):
                    previous.append(("mode", handle, mode.value))

        if not previous:
            return

        def restore() -> None:
            for item in previous:
                if item[0] == "cp":
                    kernel32.SetConsoleOutputCP(item[1])
                else:
                    kernel32.SetConsoleMode(item[1], item[2])

        atexit.register(restore)
    except (AttributeError, ImportError, OSError):
        pass

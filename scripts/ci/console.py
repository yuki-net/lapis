"""CI 用 script の出力エンコーディングを揃える。

Windows の CI runner は既定コードページが cp1252 のため、日本語を含む
ログを print すると UnicodeEncodeError で step ごと落ちる。どの script から
呼ばれても UTF-8 + 置換で書き出せるようにする。

ローカルの Windows では逆に、書き出す側を UTF-8 にしてもコンソールが
cp932 のままだと日本語が化ける。コンソールの出力コードページも UTF-8 に
揃える（失敗しても致命的ではない）。
"""

import os
import sys


def configure() -> None:
    """stdout / stderr を UTF-8 に切り替える。失敗しても致命的ではない。"""
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):
            pass

    if os.name == "nt":
        _set_console_utf8()


def _set_console_utf8() -> None:
    """コンソールに UTF-8 を解釈させる。

    パイプやリダイレクトではコンソールが無く、失敗するだけなので無視する。
    コードページはコンソール全体で共有されるため、終了時に元へ戻す。
    """
    try:
        import atexit
        import ctypes

        kernel32 = ctypes.windll.kernel32
        previous = kernel32.GetConsoleOutputCP()
        if not previous or previous == 65001:
            return
        if not kernel32.SetConsoleOutputCP(65001):
            return
        atexit.register(kernel32.SetConsoleOutputCP, previous)
    except (AttributeError, ImportError, OSError):
        pass

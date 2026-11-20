"""CI 用 script の出力エンコーディングを揃える。

Windows の CI runner は既定コードページが cp1252 のため、日本語を含む
ログを print すると UnicodeEncodeError で step ごと落ちる。どの script から
呼ばれても UTF-8 + 置換で書き出せるようにする。
"""

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

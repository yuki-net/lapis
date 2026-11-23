"""cargo の JSON 診断を、人間向けログ・GitHub 注釈・集計に変える。

`cargo clippy --message-format=json` は 1 行 1 JSON で診断を出す。ここでは
`compiler-message` だけを取り出し、rustc が描画した `rendered` をログに流す。
あわせて診断をファイル・行・ルール付きの注釈にし、ルール別・ファイル別の
件数を集計する。

`-D warnings` は使わない。警告で cargo を止めると、そのクレートに依存する
クレートの診断が出なくなるため。診断が 1 件でもあれば step を失敗にする
（collect-then-fail）。
"""

import json
from collections import Counter
from dataclasses import dataclass


def escape(text: str) -> str:
    """GitHub の注釈メッセージ用に制御文字を逃がす。"""
    return (text or "").replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def escape_property(text: str) -> str:
    """注釈のプロパティ（file や title）用。`,` と `:` も区切りと衝突する。"""
    return escape(text).replace(":", "%3A").replace(",", "%2C")


@dataclass(frozen=True)
class Diagnostic:
    level: str
    code: str
    message: str
    file: str
    line: int
    column: int
    end_line: int
    end_column: int

    def annotation(self) -> str:
        level = "error" if self.level == "error" else "warning"
        head = f"{self.code}: {self.message}" if self.code else self.message
        if not self.file:
            return f"::{level}::{escape(head)}"
        properties = [f"file={escape_property(self.file)}", f"line={self.line}"]
        if self.column:
            properties.append(f"col={self.column}")
        if self.end_line:
            properties.append(f"endLine={self.end_line}")
        if self.end_column:
            properties.append(f"endColumn={self.end_column}")
        if self.code:
            properties.append(f"title={escape_property(self.code)}")
        return f"::{level} {','.join(properties)}::{escape(self.message)}"


class Collector:
    """JSON 行を逐次受け取り、ログ用テキストと診断を溜める。"""

    def __init__(self):
        self._buffer = ""
        self._text: list[str] = []
        self._items: list[Diagnostic] = []
        self._seen: set[tuple] = set()
        self._compiled: set[str] = set()

    @property
    def text(self) -> str:
        return "".join(self._text)

    @property
    def items(self) -> list[Diagnostic]:
        return list(self._items)

    def failed(self) -> bool:
        """警告・エラーが 1 件でもあれば True（-D warnings の代わり）。"""
        return bool(self._items)

    def feed(self, chunk: str) -> str:
        """追記された出力を受け取り、ログに出せる行を返す。

        行が途中で切れることがあるので、最後の不完全な行は次回に回す。
        """
        self._buffer += chunk
        lines = self._buffer.split("\n")
        self._buffer = lines.pop()
        text = "".join(self._line(line) for line in lines)
        self._text.append(text)
        return text

    def flush(self) -> str:
        """末尾の不完全な行を処理する（プロセス終了時に呼ぶ）。"""
        line, self._buffer = self._buffer, ""
        if not line:
            return ""
        text = self._line(line)
        self._text.append(text)
        return text

    def annotations(self) -> list[str]:
        return [item.annotation() for item in self._items]

    def summary(self) -> str:
        if not self._items:
            return ""
        errors = sum(1 for item in self._items if item.level == "error")
        warnings = len(self._items) - errors
        rules = Counter(item.code or "rustc" for item in self._items)
        files = Counter(item.file or "(no file)" for item in self._items)
        lines = [
            "### Lint diagnostics",
            "",
            f"- errors: {errors}",
            f"- warnings: {warnings}",
            "",
            "#### ルール別（上位 10 件）",
            "",
            "| rule | count |",
            "| --- | --- |",
        ]
        lines += [f"| `{name}` | {count} |" for name, count in rules.most_common(10)]
        lines += ["", "#### ファイル別（上位 10 件）", "", "| file | count |", "| --- | --- |"]
        lines += [f"| `{name}` | {count} |" for name, count in files.most_common(10)]
        return "\n".join(lines)

    def _line(self, line: str) -> str:
        if not line.strip():
            return ""
        try:
            data = json.loads(line)
        except json.JSONDecodeError:
            # cargo 自体のエラーなど、JSON でない行はそのまま見せる。
            return line + "\n"

        reason = data.get("reason")
        if reason == "compiler-artifact":
            # JSON にはビルド開始イベントが無いので、完了したものだけ進捗として出す。
            name = (data.get("target") or {}).get("name")
            if name and name not in self._compiled:
                self._compiled.add(name)
                return f"   Compiling {name}\n"
            return ""
        if reason != "compiler-message":
            return ""

        message = data.get("message") or {}
        rendered = message.get("rendered") or ""
        if message.get("level") in ("warning", "error"):
            self._record(message)
        if rendered and not rendered.endswith("\n"):
            rendered += "\n"
        return rendered

    def _record(self, message: dict) -> None:
        spans = message.get("spans") or []
        span = next((span for span in spans if span.get("is_primary")), None)
        if span is None and spans:
            span = spans[0]
        span = span or {}
        code = (message.get("code") or {}).get("code") or ""
        text = message.get("message") or ""
        item = Diagnostic(
            level=message.get("level") or "",
            code=code,
            message=text,
            file=(span.get("file_name") or "").replace("\\", "/"),
            line=int(span.get("line_start") or 0),
            column=int(span.get("column_start") or 0),
            end_line=int(span.get("line_end") or 0),
            end_column=int(span.get("column_end") or 0),
        )
        key = (item.level, item.code, item.file, item.line, item.column, item.message)
        if key in self._seen:
            return
        self._seen.add(key)
        self._items.append(item)

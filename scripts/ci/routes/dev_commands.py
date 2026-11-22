"""Dev Commands route: 開発起動コマンドと dev-session の検証。"""

import sys

from scripts.ci.model import PYTHON_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


ROUTE = Route(
    key="dev_commands",
    name="Dev Commands",
    runs_on=("windows",),
    timeout_minutes=20,
    python=PYTHON_VERSION,
    rust=True,
    rust_components="rustfmt",
    steps=(
        Step("Test dev commands", py("-m", "unittest", "discover", "-s", "scripts", "-p", "test_dev_commands.py")),
        Step("Check dev-session formatting", ["cargo", "fmt", "--manifest-path", "scripts/dev-session/Cargo.toml", "--", "--check"]),
        Step("Test dev session", py("-m", "unittest", "discover", "-s", "scripts", "-p", "test_dev_session.py")),
    ),
)

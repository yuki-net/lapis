"""Site Docs route: 利用者向けドキュメントサイト（Astro Starlight）のビルド。

サイトごとに route を分ける。触っていないサイトは path routing で選ばれない
ので、CI はそのサイトをビルドしない。ホームサイトを足すときも同じ形で
`sites_home` を追加する。
"""

from scripts.ci.model import NODE_VERSION, Route, Step

ROUTE = Route(
    key="sites_docs",
    name="Site Docs",
    runs_on=("linux",),
    timeout_minutes=15,
    node=NODE_VERSION,
    node_cache="npm",
    node_cache_path="sites/docs/package-lock.json",
    working_directory="sites/docs",
    steps=(
        Step("Install dependencies", ["{npm}", "ci"]),
        Step("Build Site Docs", ["{npm}", "run", "build"]),
    ),
)

"""Mobile route: KMP のユニットテストと Android のビルド。

Gradle wrapper は OS でファイル名が違う（`./gradlew` / `gradlew.bat`）ため
`{gradlew}` を使う。wrapper は git 上で実行可能にしてあるので、Linux でも
chmod は要らない。
"""

from scripts.ci.model import JAVA_VERSION, Route, Step

ROUTE = Route(
    key="mobile",
    name="Mobile",
    runs_on=("linux", "windows"),
    target="Android",
    timeout_minutes=45,
    java=JAVA_VERSION,
    gradle=True,
    working_directory="clients/mobile",
    steps=(
        Step(
            "Test shared KMP code",
            ["{gradlew}", ":sharedUi:testDebugUnitTest", "--stacktrace"],
            retries=2,
            retry_delay_seconds=15,
        ),
        Step(
            "Build Android app",
            ["{gradlew}", ":androidApp:assembleDebug", "--stacktrace"],
            retries=2,
            retry_delay_seconds=15,
        ),
    ),
)

"""Mobile route: KMP のユニットテストと Android のビルド。"""

from scripts.ci.model import JAVA_VERSION, Route, Step

ROUTE = Route(
    key="mobile",
    name="Mobile",
    runs_on=("linux",),
    target="Android",
    timeout_minutes=45,
    java=JAVA_VERSION,
    gradle=True,
    working_directory="clients/mobile",
    steps=(
        Step("Make Gradle wrapper executable", ["chmod", "+x", "./gradlew"], requires=("posix",)),
        Step(
            "Test shared KMP code",
            ["./gradlew", ":sharedUi:testDebugUnitTest", "--stacktrace"],
            retries=2,
            retry_delay_seconds=15,
        ),
        Step(
            "Build Android app",
            ["./gradlew", ":androidApp:assembleDebug", "--stacktrace"],
            retries=2,
            retry_delay_seconds=15,
        ),
    ),
)

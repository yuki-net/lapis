"""追跡ファイルに秘密鍵や .env が混ざっていないか検査する。

`.gitignore` は事故防止のバックストップで、AI は gitignore されたファイルも
読める。追跡対象そのものに秘密が入らないことを CI で固定する。
"""

import subprocess
import unittest

from scripts.ci.model import ROOT

# 追跡されていたら落とす拡張子・ファイル名・ディレクトリ。
FORBIDDEN_SUFFIXES = (".pem", ".key", ".p12", ".pfx", ".keystore")
FORBIDDEN_NAMES = {".env", "id_rsa", "id_ed25519", "credentials", "secrets.toml"}
FORBIDDEN_PARTS = {".secrets"}


def tracked_paths():
    output = subprocess.check_output(
        ["git", "ls-files"],
        cwd=ROOT,
        text=True,
        encoding="utf-8",
    )
    return [line for line in output.splitlines() if line]


class TrackedSecretTests(unittest.TestCase):
    def test_no_tracked_secret_files(self):
        offenders = []
        for path in tracked_paths():
            name = path.rsplit("/", 1)[-1]
            if (
                name.endswith(FORBIDDEN_SUFFIXES)
                or name in FORBIDDEN_NAMES
                or any(part in FORBIDDEN_PARTS for part in path.split("/"))
            ):
                offenders.append(path)
        self.assertEqual(
            offenders,
            [],
            "秘密ファイルを追跡しない: " + ", ".join(offenders),
        )


if __name__ == "__main__":
    unittest.main()

"""開発Host停止が他worktreeや実行中の画面を巻き込まないことを検査する。"""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class WorktreeCleanupTests(unittest.TestCase):
    def test_worktree_removal_removes_generated_state_and_build_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repository = root / "repo"
            repository.mkdir()

            def git(*arguments):
                subprocess.run(
                    ["git", "-c", "core.hooksPath=", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", *arguments],
                    cwd=repository, check=True, capture_output=True,
                )

            git("init")
            (repository / ".gitignore").write_text("/target/\n", encoding="utf-8")
            git("add", ".gitignore")
            git("commit", "-m", "fixture")
            worktree = root / "worktree with spaces"
            git("worktree", "add", "--detach", str(worktree))
            for relative in ("target/debug/lapis.exe", "target/dev-state/settings-v1.json"):
                generated = worktree / relative
                generated.parent.mkdir(parents=True, exist_ok=True)
                generated.write_text("fixture", encoding="utf-8")
            git("worktree", "remove", "--force", str(worktree))
            self.assertFalse(worktree.exists())
            self.assertTrue((repository / ".gitignore").is_file())


@unittest.skipUnless(shutil.which("powershell"), "Windows PowerShellで検証")
class StopDevHostTests(unittest.TestCase):
    def test_only_owned_host_can_be_stopped(self):
        script = Path(__file__).resolve().with_name("stop_dev_host.ps1")
        harness = r'''
param([string]$Target, [string]$Scenario)
$ErrorActionPreference = 'Stop'
$global:devTestkilled = 0
$binary = [IO.Path]::GetFullPath((Join-Path (Split-Path $Target) '../target/debug/lapis.exe'))
$started = [datetime]'2026-01-01T00:00:00Z'
$global:devTestitem = [pscustomobject]@{
    ExecutablePath = $binary; ProcessId = 123; CreationDate = $started
    CommandLine = 'lapis.exe --managed-host "a workspace"'
}
$global:devTestfake = [pscustomobject]@{Handle = 1; StartTime = $started; Path = $binary}
$global:devTestfake | Add-Member ScriptMethod Kill { $global:devTestkilled++ }
$global:devTestfake | Add-Member ScriptMethod WaitForExit {}
$global:devTestfake | Add-Member ScriptMethod Dispose {}
switch ($Scenario) {
    'other' { $global:devTestitem.ExecutablePath = 'C:\other-worktree\target\debug\lapis.exe' }
    'ui' { $global:devTestitem.CommandLine = 'lapis.exe --state-directory target/dev-state' }
    'mixed' {
        $global:devTestitem = @(
            $global:devTestitem
            [pscustomobject]@{
                ExecutablePath = $binary; ProcessId = 456; CreationDate = $started
                CommandLine = 'lapis.exe --state-directory target/dev-state'
            }
        )
    }
    'reused' { $global:devTestfake.StartTime = $started.AddSeconds(1) }
    'gone' { $global:devTestfake = $null }
}
$recordPath = Join-Path (Split-Path $Target) '../target/dev-state/hosts/local-v1.host'
New-Item -ItemType Directory -Force -Path (Split-Path $recordPath) | Out-Null
Add-Type -AssemblyName System.Security
$registeredPid = if ($Scenario -eq 'foreign') { 456 } else { 123 }
$record = @{version=1; key='local-v1'; pid=$registeredPid; started_at=$started.ToUniversalTime().ToFileTimeUtc().ToString()} | ConvertTo-Json
$bytes = [Security.Cryptography.ProtectedData]::Protect(
    [Text.Encoding]::UTF8.GetBytes($record), [Text.Encoding]::UTF8.GetBytes('local-v1'),
    [Security.Cryptography.DataProtectionScope]::CurrentUser)
[IO.File]::WriteAllBytes($recordPath, $bytes)
if ($Scenario -eq 'missing') { [IO.File]::Delete($recordPath) }
function Get-CimInstance { param($ClassName, $Filter) return $global:devTestitem }
function Get-Process { param($Id, $ErrorAction) return $global:devTestfake }
$failed = $false
$message = ''
try { & $Target } catch { $failed = $true; $message = $_.Exception.Message }
if ($Scenario -eq 'missing' -and $message -ne '開発用Local Hostの記録ファイルが存在しないか、サイズが上限を超えているため停止しません。') {
    throw 'Missing record error is unreadable or incorrect'
}
if ($Scenario -eq 'foreign' -and $message -ne '開発用Local Hostの記録と実行中のprocessが一致しないため停止しません。') {
    throw 'Mismatched record error is unreadable or incorrect'
}
$expected = if ($Scenario -eq 'host') { 1 } else { 0 }
if ($global:devTestkilled -ne $expected) { throw "unexpected termination: $Scenario" }
if ($failed -ne ($Scenario -in @('ui', 'mixed', 'reused', 'foreign', 'missing'))) { throw "unexpected result: $Scenario" }
'''
        with tempfile.TemporaryDirectory() as directory:
            fixture_script = Path(directory) / "scripts" / script.name
            fixture_script.parent.mkdir()
            shutil.copyfile(script, fixture_script)
            path = Path(directory) / "host harness.ps1"
            path.write_text(harness, encoding="utf-8-sig")
            for scenario in ("host", "other", "ui", "mixed", "reused", "gone", "foreign", "missing"):
                with self.subTest(scenario=scenario):
                    result = subprocess.run(
                        ["powershell", "-NoProfile", "-NonInteractive", "-File", str(path), str(fixture_script), scenario],
                        capture_output=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))


if __name__ == "__main__":
    unittest.main()

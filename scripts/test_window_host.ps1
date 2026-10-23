param([string]$Binary = (Join-Path $PSScriptRoot '../target/debug/lapis.exe'))
$ErrorActionPreference = 'Stop'
$resolvedBinary = (Resolve-Path -LiteralPath $Binary).Path
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('lapis-window-host-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
Set-Content -LiteralPath (Join-Path $fixture 'one.md') -Encoding utf8 -Value '# Window Host fixture'
$log = Join-Path $fixture 'smoke.log'
$process = $null
try {
    $stateDirectory = Join-Path $fixture 'state'
    $process = Start-Process -FilePath $resolvedBinary -ArgumentList @('--state-directory', ('"' + $stateDirectory + '"'), '--workspace', ('"' + $fixture + '"'), '--open-file', 'one.md', '--window-host-smoke') -WindowStyle Hidden -RedirectStandardError $log -PassThru
    $processHandle = $process.Handle
    # テスト専用の段階通知を待つ。製品側の定期ポーリングではない。
    foreach ($marker in @('baseline sample begin', 'idle sample begin')) {
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        while (-not ((Get-Content -Raw -LiteralPath $log -ErrorAction SilentlyContinue) -like "*$marker*")) {
            if ($process.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw "Smoke stage missing: $marker" }
            Start-Sleep -Milliseconds 100
        }
        $process.Refresh()
        $before = $process.TotalProcessorTime
        $sample = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds 2
        $process.Refresh()
        $cpu = ($process.TotalProcessorTime - $before).TotalMilliseconds / $sample.Elapsed.TotalMilliseconds * 100
        Write-Output ('{0}: CPU {1:N2}% of one core' -f $marker, $cpu)
    }
    if (-not $process.WaitForExit(30000)) { throw 'Native Window smoke timed out' }
    $output = Get-Content -Raw -LiteralPath $log
    Write-Output $output
    Write-Output ('Exit code: {0}' -f $process.ExitCode)
    if ($process.ExitCode -ne 0 -or $output -match 'panicked' -or $output -notlike '*WINDOW_HOST_SMOKE_PASS*') { throw 'Native Window smoke failed' }
    if ((Get-Content -Raw -LiteralPath (Join-Path $fixture 'one.md')) -notlike '*smoke edit*') { throw 'Detached edit was not saved' }
    Write-Output "Fixture: $fixture"
} finally {
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id }
}

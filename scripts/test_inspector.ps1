param([string]$Binary = (Join-Path $PSScriptRoot '../target/debug/lapis.exe'))
$ErrorActionPreference = 'Stop'
$resolvedBinary = (Resolve-Path -LiteralPath $Binary).Path
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('lapis-inspector-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
Set-Content -LiteralPath (Join-Path $fixture 'one.md') -Encoding utf8 -Value '# Inspector fixture'
$log = Join-Path $fixture 'smoke.log'
$previousSmoke = $env:LAPIS_INSPECTOR_SMOKE
$process = $null
try {
    $stateDirectory = Join-Path $fixture 'state'
    $env:LAPIS_INSPECTOR_SMOKE = '1'
    $process = Start-Process -FilePath $resolvedBinary -ArgumentList @('--state-directory', ('"' + $stateDirectory + '"'), '--workspace', ('"' + $fixture + '"'), '--open-file', 'one.md', '--window-host-smoke') -WindowStyle Hidden -RedirectStandardError $log -PassThru
    $processHandle = $process.Handle
    foreach ($mode in @('normal', 'visible', 'picking')) {
        $marker = "INSPECTOR sample $mode begin"
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        while (-not ((Get-Content -Raw -LiteralPath $log -ErrorAction SilentlyContinue) -like "*$marker*")) {
            if ($process.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw "Inspector stage missing: $marker" }
            Start-Sleep -Milliseconds 100
        }
        $process.Refresh()
        $before = $process.TotalProcessorTime
        $sample = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds 2
        $process.Refresh()
        $cpu = ($process.TotalProcessorTime - $before).TotalMilliseconds / $sample.Elapsed.TotalMilliseconds * 100
        Write-Output ('{0}: CPU {1:N2}% of one core; WorkingSet {2:N2} MiB; Private {3:N2} MiB' -f $mode, $cpu, ($process.WorkingSet64 / 1MB), ($process.PrivateMemorySize64 / 1MB))
    }
    if (-not $process.WaitForExit(60000)) { throw 'Inspector Native smoke timed out' }
    $output = Get-Content -Raw -LiteralPath $log
    Write-Output $output
    if ($process.ExitCode -ne 0 -or $output -match 'panicked' -or $output -notlike '*INSPECTOR_NATIVE_SMOKE_PASS*' -or $output -notlike '*WINDOW_HOST_SMOKE_PASS*') { throw 'Inspector Native smoke failed' }
    Write-Output "Fixture: $fixture"
} finally {
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id }
    $env:LAPIS_INSPECTOR_SMOKE = $previousSmoke
}

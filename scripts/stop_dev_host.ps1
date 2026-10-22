# Windowsで再ビルド・worktree削除の前に、このworktreeのLocal Hostだけを停止する。
$ErrorActionPreference = 'Stop'
$binary = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../target/debug/lapis.exe'))
$owned = @(Get-CimInstance Win32_Process -Filter "Name = 'lapis.exe'" | Where-Object {
    $_.ExecutablePath -and [string]::Equals($_.ExecutablePath, $binary, [StringComparison]::OrdinalIgnoreCase)
})
$hosts = @($owned | Where-Object { $_.CommandLine -match '\s--managed-host\s' })
if ($owned.Count -ne $hosts.Count) {
    throw 'このworktreeの画面または作業プロセスが実行中です。保存して終了してから再実行してください。'
}
if ($hosts.Count -eq 0) { return }
# ??exe???????Host????????????????????
$recordPath = Join-Path $PSScriptRoot '../target/dev-state/hosts/local-v1.host'
if (-not (Test-Path -LiteralPath $recordPath) -or (Get-Item -LiteralPath $recordPath).Length -gt 131072) {
    throw '???Local Host???????????????????'
}
Add-Type -AssemblyName System.Security
$encrypted = [IO.File]::ReadAllBytes($recordPath)
$plaintext = [Security.Cryptography.ProtectedData]::Unprotect(
    $encrypted, [Text.Encoding]::UTF8.GetBytes('local-v1'), [Security.Cryptography.DataProtectionScope]::CurrentUser)
try {
    $record = [Text.Encoding]::UTF8.GetString($plaintext) | ConvertFrom-Json
    $registeredPid = $record.pid
    $registeredStart = $record.started_at
    if ($record.version -ne 1 -or $record.key -ne 'local-v1' -or
        $hosts.Count -ne 1 -or $hosts[0].ProcessId -ne $registeredPid) {
        throw '???????Host?process???????????????'
    }
} finally {
    [Array]::Clear($plaintext, 0, $plaintext.Length)
    $record = $null
}
foreach ($hostProcess in $hosts) {
    # PIDだけを使わず、取得したprocess handleと開始時刻で再利用を検査する。
    $process = Get-Process -Id $hostProcess.ProcessId -ErrorAction SilentlyContinue
    if (-not $process) { continue }
    try {
        $null = $process.Handle
        if ($process.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -ne $registeredStart -or
            [Math]::Abs(($process.StartTime.ToUniversalTime() - $hostProcess.CreationDate.ToUniversalTime()).TotalMilliseconds) -ge 1 -or
            -not [string]::Equals($process.Path, $binary, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Hostのprocess情報が変わりました。再実行してください。'
        }
        $process.Kill()
        $process.WaitForExit()
        Write-Output ('Local Hostを停止しました: {0}' -f $hostProcess.ProcessId)
    } finally {
        $process.Dispose()
    }
}

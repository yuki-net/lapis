# Windowsで再ビルド・worktree削除の前に、このworktreeのLocal Hostだけを停止する。
param([string]$Session, [string]$LocalHostDirectory)
$ErrorActionPreference = 'Stop'
# lapis-host.exe を対象とする。lapis.exe (Desktop) は対象外。
if ($Session) {
    if ($Session -notmatch '^\d+-\d+-\d+$') { throw 'session名が不正です。' }
    $sessionDirectory = Join-Path $PSScriptRoot "../target/dev-sessions/$Session"
    foreach ($path in @((Join-Path $PSScriptRoot '../target'), (Join-Path $PSScriptRoot '../target/dev-sessions'), $sessionDirectory, (Join-Path $sessionDirectory 'bin'), (Join-Path $sessionDirectory 'hosts'))) {
        $item = Get-Item -LiteralPath $path -ErrorAction Stop
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'sessionディレクトリを安全に確認できません。'
        }
    }
    $hostBinary = [IO.Path]::GetFullPath((Join-Path $sessionDirectory 'bin/lapis-host.exe'))
    $desktopBinary = [IO.Path]::GetFullPath((Join-Path $sessionDirectory 'bin/lapis.exe'))
    $recordDirectory = Join-Path $sessionDirectory 'hosts'
    if ($LocalHostDirectory) {
        $candidate = if ([IO.Path]::IsPathRooted($LocalHostDirectory)) { $LocalHostDirectory } else { Join-Path (Split-Path -Parent $PSScriptRoot) $LocalHostDirectory }
        $recordDirectory = [IO.Path]::GetFullPath($candidate)
        # 指定先だけでなく親も確認し、junction経由の別場所を拒否する。
        $directory = Get-Item -LiteralPath $recordDirectory -ErrorAction Stop
        while ($directory) {
            if ($directory -isnot [IO.DirectoryInfo] -or ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Host記録ディレクトリを安全に確認できません。'
            }
            $directory = $directory.Parent
        }
    }
    $recordPath = Join-Path $recordDirectory 'local-v1.host'
} else {
    if ($LocalHostDirectory) { throw 'LocalHostDirectoryにはSessionの指定が必要です。' }
    $hostBinary = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../target/debug/lapis-host.exe'))
    $desktopBinary = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../target/debug/lapis.exe'))
    $recordPath = Join-Path $PSScriptRoot '../target/dev-state/hosts/local-v1.host'
}
$hosts = @(Get-CimInstance Win32_Process -Filter "Name = 'lapis-host.exe'" | Where-Object {
    $_.ExecutablePath -and [string]::Equals($_.ExecutablePath, $hostBinary, [StringComparison]::OrdinalIgnoreCase)
})
# Desktopが実行中なら停止しない。
$desktopRunning = @(Get-CimInstance Win32_Process -Filter "Name = 'lapis.exe'" | Where-Object {
    $_.ExecutablePath -and [string]::Equals($_.ExecutablePath, $desktopBinary, [StringComparison]::OrdinalIgnoreCase)
})
if ($desktopRunning.Count -gt 0) {
    throw 'このworktreeの画面プロセスが実行中です。保存して終了してから再実行してください。'
}
if ($hosts.Count -eq 0) { return }
# 開発用保存先の登録と照合する。
if (-not (Test-Path -LiteralPath $recordPath) -or (Get-Item -LiteralPath $recordPath).Length -gt 131072) {
    throw '開発用Local Hostの記録ファイルが存在しないか、サイズが上限を超えているため停止しません。'
}
if ((Get-Item -LiteralPath $recordPath).Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw 'Host記録ファイルを安全に確認できません。'
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
        throw '開発用Local Hostの記録と実行中のprocessが一致しないため停止しません。'
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
            -not [string]::Equals($process.Path, $hostBinary, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Hostのprocess情報が変わりました。再実行してください。'
        }
        $process.Kill()
        $process.WaitForExit()
        Write-Output ('Local Hostを停止しました: {0}' -f $hostProcess.ProcessId)
    } finally {
        $process.Dispose()
    }
}

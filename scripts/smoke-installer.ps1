$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$testRoot = [System.IO.Path]::GetFullPath((Join-Path $repository ('..\installer-smoke-' + [guid]::NewGuid().ToString('N'))))
$workspace = [System.IO.Path]::GetFullPath((Split-Path -Parent $repository))
if (-not $testRoot.StartsWith($workspace + '\', [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Test directory is outside workspace' }
if (Test-Path -LiteralPath $testRoot) { throw 'Test directory already exists; choose a fresh workspace' }
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Get-ItemProperty -LiteralPath $runKey -Name ClipVault -ErrorAction SilentlyContinue) { throw 'Existing ClipVault startup setting; refusing to alter it' }
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{BBCA71EA-6283-43CF-8A72-25A20D3D0772}_is1'
if (Test-Path -LiteralPath $uninstallKey) { throw 'Existing ClipVault installation; refusing to replace it' }
$installer = Join-Path $repository 'dist\ClipVault-Setup-0.2.0-x64.exe'
$installLog = Join-Path $workspace 'installer-install.log'
$args = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/NOICONS', '/TASKS=""', "/DIR=`"$testRoot`"", "/LOG=`"$installLog`"")
$originalLocalAppData = $env:LOCALAPPDATA
$originalNoSync = $env:CLIPVAULT_NO_SYNC
try {
$env:LOCALAPPDATA = Join-Path $testRoot 'test-local-appdata'
$env:CLIPVAULT_NO_SYNC = '1'
$setup = Start-Process -FilePath $installer -ArgumentList $args -PassThru -WindowStyle Hidden
if (-not $setup.WaitForExit(30000)) { throw 'Installer did not finish within 30 seconds' }
if ($setup.ExitCode -ne 0) { throw "Installer exit $($setup.ExitCode)" }
$installed = Join-Path $testRoot 'ClipVault.exe'
if (-not (Test-Path -LiteralPath $installed)) { throw 'Installed EXE is missing' }
$builtHash = (Get-FileHash -LiteralPath (Join-Path $repository 'target\release\clipvault.exe')).Hash
if ((Get-FileHash -LiteralPath $installed).Hash -ne $builtHash) { throw 'Installed EXE differs from release build' }
foreach ($name in @('README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'unins000.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $testRoot $name))) { throw "Missing installed file: $name" }
}
if (-not (Test-Path -LiteralPath $uninstallKey)) { throw 'Uninstall registration missing' }
if ((Get-ItemProperty -LiteralPath $runKey -Name ClipVault -ErrorAction SilentlyContinue).ClipVault -ne "`"$installed`"") { throw 'Startup must be enabled automatically without popup arguments' }
$deadline = [DateTime]::UtcNow.AddSeconds(10)
do {
    $background = @(Get-CimInstance Win32_Process -Filter "Name='ClipVault.exe'" | Where-Object { $_.ExecutablePath -eq $installed })
    $ready = $background.Count -eq 1 -and (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'ClipVault\items'))
    if (-not $ready) { Start-Sleep -Milliseconds 100 }
} until ($ready -or [DateTime]::UtcNow -gt $deadline)
if ($background.Count -ne 1) { throw 'Installer must start exactly one background process' }
$backgroundProcess = Get-Process -Id $background[0].ProcessId
if ($backgroundProcess.MainWindowHandle -ne 0) { throw 'Installer opened a visible window' }
if (-not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'ClipVault\items'))) { throw 'Background history worker did not initialize' }
Stop-Process -Id $backgroundProcess.Id
if (-not $backgroundProcess.WaitForExit(5000)) { throw 'Test background process did not stop' }
$signin = Start-Process -FilePath $installed -PassThru -WindowStyle Hidden
if ($signin.WaitForExit(1000)) { throw 'Sign-in launch did not stay running' }
$signin.Refresh()
if ($signin.MainWindowHandle -ne 0) { throw 'Sign-in launch opened a visible window' }
Stop-Process -Id $signin.Id
$remove = Start-Process -FilePath (Join-Path $testRoot 'unins000.exe') -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -PassThru -Wait -WindowStyle Hidden
if ($remove.ExitCode -ne 0) { throw "Uninstaller exit $($remove.ExitCode)" }
if (Test-Path -LiteralPath $installed) { throw 'Installed EXE survived uninstall' }
if (Test-Path -LiteralPath $uninstallKey) { throw 'Uninstall registration survived uninstall' }
if (Get-ItemProperty -LiteralPath $runKey -Name ClipVault -ErrorAction SilentlyContinue) { throw 'Startup entry survived uninstall' }
Write-Output 'PASS: per-user install, EXE hash, automatic startup, background launch after install and at sign-in, uninstall, and startup cleanup.'
} finally {
    Get-CimInstance Win32_Process -Filter "Name='ClipVault.exe'" | Where-Object { $_.ExecutablePath -eq (Join-Path $testRoot 'ClipVault.exe') } | ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
    $uninstaller = Join-Path $testRoot 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller) { Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -Wait -WindowStyle Hidden }
    $env:LOCALAPPDATA = $originalLocalAppData
    $env:CLIPVAULT_NO_SYNC = $originalNoSync
    if ((Test-Path -LiteralPath $testRoot) -and $testRoot.StartsWith($workspace + '\', [System.StringComparison]::OrdinalIgnoreCase)) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}

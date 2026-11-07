$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$testRoot = [System.IO.Path]::GetFullPath((Join-Path $repository '..\installer-smoke'))
$workspace = [System.IO.Path]::GetFullPath((Split-Path -Parent $repository))
if (-not $testRoot.StartsWith($workspace + '\', [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Test directory is outside workspace' }
if (Test-Path -LiteralPath $testRoot) { throw 'Test directory already exists; choose a fresh workspace' }
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Get-ItemProperty -LiteralPath $runKey -Name ClipVault -ErrorAction SilentlyContinue) { throw 'Existing ClipVault startup setting; refusing to alter it' }
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{BBCA71EA-6283-43CF-8A72-25A20D3D0772}_is1'
if (Test-Path -LiteralPath $uninstallKey) { throw 'Existing ClipVault installation; refusing to replace it' }
$installer = Join-Path $repository 'dist\ClipVault-Setup-0.1.0-x64.exe'
$installLog = Join-Path $workspace 'installer-install.log'
$args = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/NOICONS', '/TASKS=""', "/DIR=`"$testRoot`"", "/LOG=`"$installLog`"")
$setup = Start-Process -FilePath $installer -ArgumentList $args -PassThru -Wait -WindowStyle Hidden
if ($setup.ExitCode -ne 0) { throw "Installer exit $($setup.ExitCode)" }
$installed = Join-Path $testRoot 'ClipVault.exe'
if (-not (Test-Path -LiteralPath $installed)) { throw 'Installed EXE is missing' }
$builtHash = (Get-FileHash -LiteralPath (Join-Path $repository 'target\release\clipvault.exe')).Hash
if ((Get-FileHash -LiteralPath $installed).Hash -ne $builtHash) { throw 'Installed EXE differs from release build' }
foreach ($name in @('README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'unins000.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $testRoot $name))) { throw "Missing installed file: $name" }
}
if (-not (Test-Path -LiteralPath $uninstallKey)) { throw 'Uninstall registration missing' }
if (Get-ItemProperty -LiteralPath $runKey -Name ClipVault -ErrorAction SilentlyContinue) { throw 'Startup should be opt-in' }
# Simulate enabling startup from the app after installing with the task unchecked.
New-ItemProperty -LiteralPath $runKey -Name ClipVault -PropertyType String -Value "`"$installed`"" | Out-Null
$remove = Start-Process -FilePath (Join-Path $testRoot 'unins000.exe') -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -PassThru -Wait -WindowStyle Hidden
if ($remove.ExitCode -ne 0) { throw "Uninstaller exit $($remove.ExitCode)" }
if (Test-Path -LiteralPath $installed) { throw 'Installed EXE survived uninstall' }
if (Test-Path -LiteralPath $uninstallKey) { throw 'Uninstall registration survived uninstall' }
if (Get-ItemProperty -LiteralPath $runKey -Name ClipVault -ErrorAction SilentlyContinue) { throw 'Startup entry survived uninstall' }
Write-Output 'PASS: silent per-user install, EXE hash, notices, uninstall registration, startup opt-in, uninstall, and startup cleanup.'

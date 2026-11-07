$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw 'Format check failed' }
    cargo clippy --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    cargo test --locked -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    cargo build --locked --release
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    New-Item -ItemType Directory -Path 'dist' -Force | Out-Null
    $metadata = cargo metadata --locked --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed' }
    $notices = [System.Text.StringBuilder]::new()
    [void]$notices.AppendLine('ClipVault third-party dependency licenses. Includes build and test dependencies for completeness.')
    foreach ($package in ($metadata.packages | Where-Object { $_.source } | Sort-Object name,version)) {
        [void]$notices.AppendLine("`n===== $($package.name) $($package.version) : $($package.license) =====")
        $packageDirectory = Split-Path -Parent $package.manifest_path
        $licenseFiles = Get-ChildItem -LiteralPath $packageDirectory -File -Recurse | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE)' }
        foreach ($licenseFile in $licenseFiles) {
            [void]$notices.AppendLine("`n--- $($licenseFile.Name) ---")
            [void]$notices.AppendLine([System.IO.File]::ReadAllText($licenseFile.FullName))
        }
    }
    [System.IO.File]::WriteAllText((Join-Path (Get-Location) 'dist\THIRD_PARTY_NOTICES.txt'), $notices.ToString())
    $compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
    if (-not (Test-Path -LiteralPath $compiler)) { throw 'Install Inno Setup 6 first.' }
    & $compiler 'installer\setup.iss'
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
    Copy-Item -LiteralPath 'target\release\clipvault.exe' -Destination 'dist\ClipVault-0.1.0-x64.exe' -Force
    $hashes = Get-ChildItem -LiteralPath 'dist' -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object Name | ForEach-Object {
        $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
        '{0}  {1}' -f $hash.Hash.ToLowerInvariant(), $_.Name
    }
    [System.IO.File]::WriteAllLines((Join-Path (Get-Location) 'dist\SHA256SUMS.txt'), $hashes)
} finally { Pop-Location }

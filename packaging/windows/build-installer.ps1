# Builds target\packages\dacode-setup.exe — the thing a Windows user downloads.
#
# Needs Inno Setup 6.3 or newer (`winget install JRSoftware.InnoSetup`). Everything else it needs is
# in the repository. Run it from anywhere:
#
#     powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1
#
# Pass -SkipBuild if `cargo build --release` has already run.

param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $SkipBuild) {
    Push-Location $root
    try { cargo build --release; if ($LASTEXITCODE -ne 0) { throw "cargo build failed" } }
    finally { Pop-Location }
}

$binary = Join-Path $root 'target\release\dacode.exe'
if (-not (Test-Path $binary)) {
    Write-Error "no $binary — build it first: cargo build --release"
}

# One version number, and it lives in Cargo.toml. An installer that names a different one than the
# binary it carries is a support question nobody can answer.
$cargo = Get-Content (Join-Path $root 'crates\dacode\Cargo.toml')
$version = ($cargo | Select-String -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

# ISCC is not on PATH after a default install, so the usual places are tried before giving up.
$iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    foreach ($candidate in @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    )) {
        if (Test-Path $candidate) { $iscc = $candidate; break }
    }
}
if (-not $iscc) {
    Write-Error "Inno Setup is not installed — winget install JRSoftware.InnoSetup"
}

New-Item -ItemType Directory -Force -Path (Join-Path $root 'target\packages') | Out-Null
& $iscc "/DAppVersion=$version" (Join-Path $PSScriptRoot 'dacode.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

Write-Host "built $(Join-Path $root 'target\packages\dacode-setup.exe') for dacode $version"

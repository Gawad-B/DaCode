# Installs dacode for the current user on Windows.
#
# Per-user, under %LOCALAPPDATA%\Programs, so it needs no administrator rights — the same reason the
# Linux installer defaults to ~/.local. Run it from a PowerShell prompt in the repository root after
# `cargo build --release`:
#
#     powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$binary = Join-Path $root 'target\release\dacode.exe'
if (-not (Test-Path $binary)) {
    Write-Error "build it first: cargo build --release"
}

$target = Join-Path $env:LOCALAPPDATA 'Programs\dacode'
New-Item -ItemType Directory -Force -Path $target | Out-Null
Copy-Item $binary (Join-Path $target 'dacode.exe') -Force

# Documentation beside it, because on Windows nobody goes looking in /usr/share/doc.
foreach ($file in 'README.md', 'CHEATSHEET.md', 'CHANGELOG.md') {
    Copy-Item (Join-Path $root $file) $target -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $target 'examples') | Out-Null
Copy-Item (Join-Path $root 'examples\init.lua') (Join-Path $target 'examples') -Force
# The whole docs tree: the README's links into it are relative, so half of it is a page of dead ends.
Copy-Item (Join-Path $root 'docs') $target -Recurse -Force

# On PATH, so `dacode` works from any terminal. The user's own PATH, not the machine's, and only if it is
# not already there — appending blindly is how PATH variables end up a kilometre long.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$target*") {
    $separator = if ($userPath -and -not $userPath.EndsWith(';')) { ';' } else { '' }
    [Environment]::SetEnvironmentVariable('Path', "$userPath$separator$target", 'User')
    Write-Host "added $target to your PATH — open a new terminal for it to take effect"
}

# A Start Menu entry, which is what makes it feel installed rather than copied.
$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\dacode.lnk'
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = Join-Path $target 'dacode.exe'
$link.WorkingDirectory = $target
$link.Description = 'A modal code editor that never stalls on your keystroke'
$link.Save()

Write-Host "installed dacode to $target"
Write-Host "settings live in $env:APPDATA\dacode\init.lua"

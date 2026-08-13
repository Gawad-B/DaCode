# vex on Windows

## Build and install

```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1
```

That puts `vex.exe` in `%LOCALAPPDATA%\Programs\vex`, adds it to your user `PATH` and makes a Start
Menu entry. No administrator rights needed.

## Where things go

| | |
|---|---|
| Settings, themes, snippets | `%APPDATA%\vex\` — `init.lua` lives here |
| Sessions and crash recovery | `%LOCALAPPDATA%\vex\` |

Configuration is under the *roaming* profile because it is yours and follows you between machines;
sessions and recovery copies are machine-local, since a recovery copy of a file that exists on one
computer is meaningless on another.

## What needs installing separately

- **A language server**, for diagnostics, hover, rename and completion — `rustup component add
  rust-analyzer`, `npm i -g typescript-language-server`, and so on. vex finds them on `PATH`,
  including `.cmd` and `.bat` shims, which is how most of them install on Windows.
- **git**, for the diff gutter. Any git in `PATH` will do.

Everything else — fonts, syntax highlighting, the folder chooser — is in the binary or in Windows.

## Diagnostics

A release build is a **GUI application**: no console window appears behind the editor. That also means
`eprintln!` output goes nowhere, so anything worth knowing is shown in the editor — the statusline says
what the language server is doing, configuration mistakes appear in a box at startup, and failed
requests report themselves.

To see the terminal output while troubleshooting, run a debug build, which keeps its console:

```powershell
cargo run -- path	oile.rs
```

## Honest status

The editor **builds and its whole test suite passes on Windows** in CI, including a live handshake with
rust-analyzer — so the language-server path, the paths module and the URI conversion are exercised on
the real platform, not merely type-checked. Getting there took three CI failures, all one root cause:
`canonicalize` returns `\\?\C:\...` on Windows, and code that mixed that form with the plain one broke
URIs, root-finding and path comparison in turn.

What is still **unverified** is everything a test cannot press: the window itself, the mouse, the shell
folder chooser, and how the editor looks. Nobody has run it there. If something is wrong it will be in
those, and this file lists where each of them lives.

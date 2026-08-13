# vex on Windows

## Install

Run **`vex-setup.exe`**, from the `vex-windows` artifact of a `build` run. It installs per-user under
`%LOCALAPPDATA%\Programs\vex` — no administrator rights, no UAC prompt — and gives you a Start Menu
entry, `vex` on your `PATH`, "Open with vex" on a folder's right-click menu, and an uninstaller that
takes the `PATH` entry back out. Your settings under `%APPDATA%\vex` survive an uninstall.

`vex.exe` is in the same artifact, but it is the *editor*, not an installer: double-clicking it opens
the editor without installing anything.

## Build it yourself

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1
```

Needs [Inno Setup](https://jrsoftware.org/isdl.php) 6.3 or newer (`winget install
JRSoftware.InnoSetup`); the script builds the release binary, reads the version out of
`crates/vex/Cargo.toml` and writes `target\packages\vex-setup.exe`. `packaging\windows\vex.iss` is the
script it compiles.

To place the binary without an installer at all:

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

A release build is a **GUI application**: no console window appears behind the editor. Because it has
no console of its own, every console program it starts — `git` for the diff gutter, `powershell` for
the folder chooser, a language server — would be given a *fresh* one, which Windows shows as a black
window flashing over the editor. They are all spawned through `vex::process::command`, which sets
`CREATE_NO_WINDOW`. Anything new that spawns a process must go through it too; `vex-lsp` keeps its own
copy of that helper because it sits below the binary in the dependency order.

Being a GUI application also means
`eprintln!` output goes nowhere, so anything worth knowing is shown in the editor — the statusline says
what the language server is doing, configuration mistakes appear in a box at startup, and failed
requests report themselves.

To see the terminal output while troubleshooting, run a debug build, which keeps its console:

```powershell
cargo run -- path\to\file.rs
```

## Honest status

The editor **builds and its whole test suite passes on Windows** in CI, including a live handshake with
rust-analyzer — so the language-server path, the paths module and the URI conversion are exercised on
the real platform, not merely type-checked. Getting there took three CI failures, all one root cause:
`canonicalize` returns `\\?\C:\...` on Windows, and code that mixed that form with the plain one broke
URIs, root-finding and path comparison in turn.

It has now been **run** there, once, and that first run found three things no test had: the settings
panel wrote a theme name the config reader would only look for as a file, so choosing a theme and
restarting reported it missing; the folder chooser passed its starting directory as the shell browser's
*root*, which confined the tree to that one folder; and every helper process opened a console window,
because a GUI application has none to lend them. All three are fixed, and the first has a test.

What is still **unverified** is everything a test cannot press: the window itself, the mouse, and how
the editor looks. If something else is wrong it will be in those, and this file lists where each of
them lives.

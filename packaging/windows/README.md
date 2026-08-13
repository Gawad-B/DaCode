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

## Honest status

The Windows-specific code paths (`%APPDATA%` handling, `PATHEXT` lookup, the shell folder chooser) are
written and type-checked, but the editor has **not been run on Windows** by its author. Seven of the
nine crates type-check against `x86_64-pc-windows-msvc`; the remaining two need a Windows C toolchain
for tree-sitter and Lua, which is what the CI workflow provides. If something is wrong there it will
be in the untested seams, and the places to look are listed above.

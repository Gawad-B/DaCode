<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo/design_handoff_vex/logo/png/paper/vex-128.png">
  <img src="logo/design_handoff_vex/logo/png/ink/vex-128.png" alt="vex" width="96">
</picture>

# vex

**A code editor that never stalls on your keystroke.**

[![build](https://github.com/Gawad-B/Vex/actions/workflows/build.yml/badge.svg)](https://github.com/Gawad-B/Vex/actions/workflows/build.yml)
[![release](https://img.shields.io/github/v/release/Gawad-B/Vex)](https://github.com/Gawad-B/Vex/releases/latest)
[![licence](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

[Install](#install) · [Documentation](docs/) · [Keys](CHEATSHEET.md) · [Changelog](CHANGELOG.md)

</div>

---

Rust, `winit` + `wgpu` + `glyphon` — drawn straight onto the GPU, with no GUI framework and no web
view. Tree-sitter highlighting, language servers over LSP, splits, a file tree, a git gutter and a
command palette. Vim's grammar if you want it, a conventional editor if you don't.

One rule is enforced above every other: **nothing unbounded ever runs on the UI thread.** That is why
a keystroke costs the same in a 100,000-line file as in a ten-line one — about 7 µs either way — and
it is asserted by a test that fails the build if it stops being true.

![vex editing a Rust file](docs/images/editor.png)

## Install

**Windows** — download [`vex-setup.exe`](https://github.com/Gawad-B/Vex/releases/latest) and run it.
It installs per-user under `%LOCALAPPDATA%\Programs\vex`, so there is no UAC prompt, and it adds a
Start Menu entry, `vex` on your `PATH`, "Open with vex" on a folder's right-click menu, and an
uninstaller.

**Debian, Ubuntu, Mint**

```sh
sudo apt install ./vex_0.4.2_amd64.deb
```

**Fedora, RHEL, openSUSE**

```sh
sudo dnf install ./vex-0.4.2-1.x86_64.rpm
```

**Any Linux, no package manager**

```sh
tar xzf vex-0.4.2-x86_64-linux.tar.gz
./vex-0.4.2-x86_64-linux/install.sh        # into ~/.local, no root needed
```

**From source** — needs Rust 1.85 or newer.

```sh
cargo build --release && packaging/linux/install.sh
```

Every artifact is on the [releases page](https://github.com/Gawad-B/Vex/releases/latest).
[Getting started](docs/getting-started.md) walks through the rest.

## Start

```sh
vex path/to/file.rs      # a file
vex path/to/project      # a folder: indexes it and opens the file tree
vex                      # nothing: reopens the last project you worked in
```

Started with no argument — from a Start Menu entry, a desktop icon, a dock — vex reopens the last
project you were in, with the files you had open. Not the current working directory, which for a
launcher is wherever the shortcut happened to point.

On first run it asks three questions — how you want to type, which theme, and whether to bring
anything over from VSCode — writes the answers to your config, and never appears again.

## You do not have to learn it first

There are buttons along the top, a right-click menu with the clipboard and language-server verbs, a
menu bar on <kbd>F10</kbd>, and a settings panel you change by clicking. Every one of them prints the
shortcut for what it does, so the mouse teaches the keyboard and then gets out of the way.

|  |  |
|---|---|
| <kbd>F1</kbd> | every key that works *right now*, in the grammar you actually chose |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>P</kbd> | every command, searchable, with its key beside it |
| <kbd>ctrl</kbd>+<kbd>,</kbd> | settings, including which of default, `init.lua` or import last set each one |

<img src="docs/images/commands.png" alt="The command palette" width="49%"> <img src="docs/images/settings.png" alt="The settings panel" width="49%">

**With Vim keys off you get a real conventional editor, not a modal one with the modes hidden.**
<kbd>ctrl</kbd>+<kbd>D</kbd> selects the next occurrence, <kbd>alt</kbd>+<kbd>↑</kbd> moves a line,
<kbd>ctrl</kbd>+<kbd>/</kbd> comments, double-click takes a word, alt-click adds a cursor, and copy
and paste go through the system clipboard. Vim reaches those by composition (`ddp`, `yyp`); with the
grammar off there is nothing to compose with, so they are bindings of their own.

**With Vim keys on** the grammar composes as you expect — `3dw`, `d2f)`, `ci"`, `"ayy`, `.` to
repeat — and vertical motion follows *display* rows, so it moves through soft-wrapped lines the way
they look.

Every key both modes offer comes from [one table in the source][commands], which is also what the
palette, the help panel and the menus read. A test presses each of them through the real grammar, so
a shortcut this documentation claims that did nothing would fail the build.

[commands]: crates/vex-input/src/commands.rs

## Language support

Install a language server and vex drives it over LSP directly — or ask the editor to install it for
you, from the command palette. There is no marketplace and no extension runtime: **extension code is
never run**, because a JavaScript runtime is most of what makes VSCode heavy. Extension *data* —
themes, snippets, keybindings, language configuration — imports fine.

Rust, Python, TypeScript, JavaScript, C, C++, Go, Lua and Dart are recognised out of the box, and
[you can name your own](docs/guides/language-servers.md). Completion carries each suggestion's type
and brings its import with it; formatting works through the server when it can and through `ruff`,
`prettier`, `gofmt` or `clang-format` when it cannot.

## Documentation

| | |
|---|---|
| **[Getting started](docs/getting-started.md)** | install, first run, and editing your first file |
| **[Guides](docs/guides/)** | keymaps, language servers, configuration, themes, migrating from VSCode |
| **[Reference](docs/reference/)** | [every setting](docs/reference/settings.md), [every flag and variable](docs/reference/cli.md), [every key](CHEATSHEET.md) |
| **[Architecture](docs/explanation/architecture.md)** | how it is put together, and why it stays responsive |
| **[PLAN.md](PLAN.md)** | the full design record, including the decisions that turned out wrong |
| **[CONTRIBUTING.md](CONTRIBUTING.md)** | building, testing, and the conventions that hold |

## Status

Used daily by its author on Linux, and run on Windows since 0.3.0 — where real use immediately found
things no test could, which is what 0.4.1 and 0.4.2 are both made of. **Nobody has ever run vex on macOS.**

Deliberate omissions, with reasons in [PLAN.md](PLAN.md): no integrated terminal, no git blame, no
structural text objects, no macOS packaging. Known limitations, stated rather than hidden: buffer
search is still linear in the document, opening a file is still synchronous (though bounded), and
only the *active* buffer gets a file watcher, a git diff and a language server.

## Licence

MIT — see [LICENSE](LICENSE). The bundled fonts (Fira Code, IBM Plex Sans, Instrument Serif) are under the SIL Open
Font License 1.1; see `crates/vex-render/fonts/`. The logo is in `logo/`, with its usage rules.

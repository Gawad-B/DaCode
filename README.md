# vex

A code editor. Rust, `winit` + `wgpu` + `glyphon` — no GUI framework, no web view.

Vim's grammar if you want it, a conventional editor if you don't — and the second one is a real
editor, not the first with the modes taken out: `ctrl-d` selects the next occurrence, `alt-↑` moves a
line, `ctrl-/` comments, double-click takes a word, alt-click adds a cursor, and copy and paste use
the system clipboard. Tree-sitter highlighting, language servers, splits, a file tree and a git
gutter. One rule is enforced above the others: **nothing unbounded ever runs on the UI thread**, which
is why a keystroke costs the same in a 100,000-line file as in a ten-line one.

**You do not have to learn it first, and you do not have to use the keyboard at all.** There are
buttons along the top — open, files, find, save, settings, commands, keys — a right-click menu with
cut, copy, paste and the language-server verbs, a menu bar on `F10`, and a settings panel you change
by clicking. Every one of them prints the shortcut for what it does, so the mouse teaches the
keyboard and then gets out of the way. When you want the keys: `F1` lists the ones that work right
now in whichever grammar you chose, `ctrl-shift-P` searches every command, and a half-typed chord
shows what can follow it.

`PLAN.md` has the architecture and the reasoning, including the decisions that turned out wrong.

**Status:** used daily by its author on Linux, and now run on Windows — where the first session found
three things no test could (themes chosen in the panel would not load again, the folder chooser could
not leave the directory it opened in, and helper processes flashed console windows). All three are
fixed and the installer is a real installer; `packaging/windows/README.md` has the detail. No
integrated terminal; see the end of M7 in `PLAN.md` for why it is deferred rather than forgotten.

---

## Install

**Linux, from source**

```sh
cargo build --release
packaging/linux/install.sh              # into ~/.local — no root needed
packaging/linux/install.sh /usr/local   # or system-wide
```

**Linux, as a package**

```sh
cargo build --release
packaging/build-deb.sh     # Debian, Ubuntu, Mint …
packaging/build-rpm.sh     # Fedora, RHEL, openSUSE — needs rpm-build installed
```

**Windows**

Download `vex-setup.exe` from the build artifacts and run it: it installs per-user under
`%LOCALAPPDATA%\Programs\vex`, so there is no UAC prompt, and it puts vex on your `PATH`, in the Start
Menu and on a folder's right-click menu. To build that installer yourself (needs Inno Setup):

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1
```

Or skip the installer and just place the binary:

```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1
```

Either installer places the binary, the desktop entry and the icon theme. Details, including why the
Linux desktop entry needs an absolute `Exec` path, are in `packaging/README.md`.

## Run

```sh
vex path/to/file.rs      # a file
vex path/to/project      # a folder: indexes it and opens the file tree
vex                      # nothing: reopens the last project you worked in
```

Started with no argument — from a Start Menu entry, a desktop icon, a dock — vex reopens **the last
project you were in**, with the files you had open. Not the current working directory, which for a
launcher is wherever the shortcut happened to point.

Naming a **folder** makes it the project — and the folder you name is the root even if it sits inside a
larger repository, because asking for one crate should not index the whole workspace. Inside the
editor, `ctrl-k o` opens your desktop's folder chooser and `:e some/dir` switches project without
closing what you have open.

Debug builds are usable — `opt-level = 1` is set for exactly that reason — but the release binary is
what the performance figures refer to.

## First run

With no configuration present, vex asks three questions before anything else:

1. **How do you want to type?** Conventional keys, or Vim's.
2. **Which theme?** Five live previews — each card renders the same code in its own theme — rather than
   five names.
3. **Anything to bring over from VSCode?** A checklist; nothing is ticked by default.

It writes the answers to your config and never appears again. `esc` skips it, and skipping writes
nothing, so it returns next launch rather than disappearing because of one stray keypress.
`--no-welcome` starts without it.

## Keys

**Full list: [CHEATSHEET.md](CHEATSHEET.md)** — every key in it is checked against the real grammar by
`cargo run -p vex-input --example cheatsheet`, which fails if one stops resolving.

**Not a Vim user?** First row of the settings panel (`ctrl-,`), or `vim_keys = false`. You get a
conventional editor, not a modal one with the modes hidden:

| Key | |
|---|---|
| `ctrl-c` · `ctrl-x` · `ctrl-v` | copy · cut · paste, through the **system** clipboard |
| `ctrl-s` · `ctrl-shift-s` | save · save as (or turn on autosave and never think about it) |
| `ctrl-d` · `ctrl-shift-l` | select the next copy of this word · every copy |
| `alt-↑↓` · `shift-alt-↑↓` | move these lines · copy them |
| `ctrl-/` · `ctrl-shift-k` | comment · delete the line |
| `tab` · `shift-tab` | indent · outdent a selection |
| `ctrl-backspace` | delete a word |
| `F2` · `F12` · `F8` | rename · go to definition · next problem |
| double-click · triple-click | select the word · the line |
| alt-click · alt-drag | another cursor · a rectangular selection |
| ctrl-click | go to the definition |

**With Vim keys on**, the grammar composes as you expect: `3dw`, `d2f)`, `ci"`, `"ayy`, `.` to repeat.
Vertical motion follows *display* rows, so it moves through soft-wrapped lines the way they look.

Either way, these are vex's own and always work:

| Key | |
|---|---|
| `F1` | every key that works right now, filterable |
| `ctrl-shift-p` | every command, searchable, with its key beside it |
| `F10` · right-click | menu bar · context menu |
| `ctrl-p` · `ctrl-b` | file palette · buffer palette |
| `ctrl-e` | file tree |
| `ctrl-,` | settings |
| `ctrl-f` · `ctrl-shift-f` | find in this file · in every file |
| `ctrl-k o` | open a folder |
| `ctrl-w v` · `ctrl-w w` | split · move between panes |
| `ctrl-s` · `ctrl-q` | save · quit |

Every key in the two tables above comes from one table in the source — `vex_input::commands` — which
is also what the palette, the help panel and the menus read, and a test presses each of them through
the real grammar. A shortcut documented here that did nothing would fail the build.

## Settings

`ctrl-,` — or the *settings* button — opens a panel with every option, its value, and the part a file
cannot tell you: **which of default, `init.lua` or imported last set it**. Click a row to change it,
or `h`/`l`; `w` writes the panel's values to `init.lua`.

| | |
|---|---|
| `vim_keys` | modal editing, or typing that always inserts |
| `font_size` · `tab_width` · `soft_wrap` | the obvious three |
| `theme` | the five built in, cycled live |
| `line_numbers` | absolute, relative (Vim's), or none — defaults to absolute without Vim keys |
| `sticky_scroll` · `inlay_hints` | pin block heads · show inferred types |
| `format_on_save` | reformat on every save — through the language server, or a formatter |
| `autosave` | write the file a moment after you stop typing |

Autosave waits for a **pause in typing** rather than running on a timer, so what reaches the disk is
always a state you stopped at — and with `format_on_save` on, it will not reformat under your hands
mid-word.

The panel also holds the VSCode import checklist and a **load a theme file…** row, so neither needs a
terminal.

Everything is also a Lua file, which is the same file the panel writes:

```sh
mkdir -p ~/.config/vex && cp examples/init.lua ~/.config/vex/init.lua
```

`examples/init.lua` documents every option. Saving it **reloads immediately** — theme, metrics and
bindings change in place and nothing about the open documents is touched. A mistake gets a box at
startup with a did-you-mean, and never prevents the editor from opening.

```lua
vex.set { vim_keys = false, theme = "rose-pine", font_size = 14 }
vex.map("normal", "<leader>w", "save")
```

`VEX_CONFIG_DIR=/tmp/somewhere vex` runs against a throwaway config.

### Themes

Five ship in the binary: **dark**, **light**, **rose-pine**, **rose-pine-moon**, **amoled** (true
black). Cycle them in the settings panel, or `vex.set { theme = "amoled" }`.

Or bring one you found, in any of four formats — VSCode JSON, TextMate `.tmTheme`, base16 YAML, or
vex's own TOML:

```sh
vex import --palette ~/Downloads/gruvbox.yaml
vex import --openvsx zhuangtongfa.material-theme --theme
```

Both convert it to vex's perceptual (OKLCH) palette and save it under `themes/`, usable as
`vex.set { theme = "gruvbox" }`. The settings panel's last row does the same thing and applies it at
once.

### Importing from VSCode

```sh
vex import --help                      # every source and item
vex import --from-vscode --list        # what your install offers, importing nothing
vex import --from-vscode --settings --theme --keybindings --snippets
```

Opt-in per item: nothing is imported unless you name it. **Extension code is never run** — extensions
are JavaScript against the `vscode` API and vex has no JS runtime, by design, since that runtime is
most of what makes VSCode heavy. Extension *data* — themes, snippets, keybindings, language
configuration — imports; anything whose value is its own JavaScript does not. §5a of `PLAN.md` has the
full account.

Language support is not extensions here: install a language server and vex drives it over LSP
directly. It looks for these, in this order, and starts the first one it finds:

| Language | Servers vex looks for | Install |
|---|---|---|
| Rust | `rust-analyzer` | `rustup component add rust-analyzer` |
| Python | `pyright-langserver`, `basedpyright-langserver`, `pylsp` | `npm i -g pyright` |
| TypeScript, JavaScript | `typescript-language-server` | `npm i -g typescript-language-server typescript` |
| C, C++ | `clangd` | install LLVM |
| Go | `gopls` | `go install golang.org/x/tools/gopls@latest` |
| Lua | `lua-language-server` | your package manager |
| Dart | `dart` | the Dart SDK |

A language server is a separate program from the language itself — having Python installed does not
give you a Python language server. When vex cannot find one it says which language it wanted and how
to install it, rather than naming a binary you have never heard of. The **settings panel** lists
every language above with where its server was found, or what would install it.

What that buys you, beyond diagnostics and go-to-definition: `ctrl-space` completes with each
suggestion's **type** beside it, in the order the server ranked them, and accepting one brings its
**import** — completing `HashMap` in a file that has no `use std::collections::HashMap` adds that
line as part of the same undo step. Snippet completions land expanded, so `println!` gives you
`println!()` with the cursor already inside. `ctrl-shift-o` opens the file as a searchable outline,
`ctrl-t` searches every symbol in the project.

## Formatting

`gf`, or `format_on_save`. A language server does it when it can — rust-analyzer runs rustfmt, gopls
runs gofmt — and otherwise vex runs a formatter itself, which is what a plain JavaScript, CSS or
Markdown project needs and what Python needs too, since pyright does not format at all.

| Language | Formatters vex looks for |
|---|---|
| Rust | `rustfmt` |
| JavaScript, TypeScript | `prettier`, `biome` |
| Python | `ruff`, `black` |
| C, C++ | `clang-format` |
| Go | `gofmt` |
| JSON, CSS, HTML, Markdown, YAML | `prettier` |

Or name your own — the same shape as `vex.server`:

```lua
vex.formatter("python", { command = ".venv/bin/black", args = { "-" } })
vex.formatter("nix", { command = "nixfmt", extensions = { "nix" } })
```

It runs on a worker thread, from the project root so it finds your `.prettierrc` or
`pyproject.toml`, and text goes in on stdin and comes back on stdout — never the path, because a
formatter handed the file would rewrite it behind a buffer you have unsaved changes in. A result
computed against text you have since typed into is discarded rather than merged, and only the part
that actually changed is replaced, so the cursor stays where you left it. The settings panel lists
what was found.

**Naming your own.** The table above is a default, not a limit:

```lua
-- A project's own virtualenv. Relative paths resolve against the project root.
vex.server("python", { command = ".venv/bin/pylsp" })

-- A language vex has never heard of has to say which files it serves.
vex.server("zig", { command = "zls", extensions = { "zig", "zon" } })
```

Yours is tried **first**; vex's own list stays behind it, so naming a venv does not cost you the
fallback when that venv is not there. A command containing a slash is a path rather than a name to
look for on `PATH`, and `~/` means your home directory.

## Where files live

| | Linux | Windows |
|---|---|---|
| Config, themes, snippets | `~/.config/vex/` | `%APPDATA%\vex\` |
| Sessions, crash recovery | `~/.local/state/vex/` | `%LOCALAPPDATA%\vex\` |

Configuration and state are separate on purpose: on Windows config roams with your profile, while a
recovery copy of a file that exists on one machine would be meaningless on another.

**Sessions** remember which files were open and where the cursor was, per project, written when the
editor closes properly. **Crash recovery** is different: dirty buffers are written every two seconds
and forced before quitting, so a `kill -9` costs seconds rather than work, and reopening the file
offers the copy back. A copy older than the file on disk is discarded rather than offered.

## Development

```sh
cargo test --workspace                      # 659 tests
cargo clippy --workspace --all-targets      # clean
cargo bench -p vex -p vex-syntax
cargo run -p vex-input --example cheatsheet # verifies CHEATSHEET.md against the grammar
```

The suite includes live `rust-analyzer` tests, **skipped** when it is not installed rather than failing:
a test that fails on a machine without a dependency is testing the machine.

### Seeing the interface without a window

The whole appearance renders offscreen to a PNG. This is how the design gets checked over SSH, and it
is how most of the visual defects in this project were actually found — a transparent completion popup,
tofu boxes where a font had no glyph, a logo that came out as an X.

```sh
vex --screenshot /tmp/vex.png                       # the editor
vex --screenshot /tmp/vex.png --light               # or --theme rose-pine
vex --screenshot /tmp/vex.png --mode insert         # any mode
vex --screenshot /tmp/vex.png --split 2             # panes; 3h for stacked
vex --screenshot /tmp/vex.png --tree                # file tree
vex --screenshot /tmp/vex.png --settings            # settings panel
vex --screenshot /tmp/vex.png --welcome theme       # first-run setup, any step
vex --screenshot /tmp/vex.png --folded              # folded blocks
vex --screenshot /tmp/vex.png --git                 # git gutter marks
vex --screenshot /tmp/vex.png --palette an          # command palette
vex --screenshot /tmp/vex.png --grep Anchor         # project search
vex --screenshot /tmp/vex.png --complete ch         # completion popup
vex --screenshot /tmp/vex.png --diagnostics-list    # diagnostics list
vex --screenshot /tmp/vex.png --splash --empty      # cold start, empty state
vex --screenshot /tmp/vex.png --help-panel          # the F1 key list
vex --screenshot /tmp/vex.png --commands            # every command, searchable
vex --screenshot /tmp/vex.png --menu                # the menu bar
vex --screenshot /tmp/vex.png --context-menu        # a right-click
vex --screenshot /tmp/vex.png --which-key '<c-w>'   # what can follow a chord
```

CI renders all of these on every push and keeps the PNGs as an artifact, so the check applies to
anyone without a GPU to hand. It earned that on its first run: the help panel drew its entire list
into a thirty-pixel strip at the foot of its card, because `split_bottom` returns the bottom piece
first and `split_top` returns the top — invisible in the code, obvious in the picture.

### Environment variables

| Variable | Effect |
|---|---|
| `VEX_CONFIG_DIR` | Configuration somewhere other than the platform default |
| `VEX_STATE_DIR` | Sessions and recovery elsewhere |
| `VEX_LSP_TRACE=1` | Log every message to and from the language server |
| `VEX_NO_SYSTEM_FONTS=1` | Bundled fonts only — faster startup, reproducible captures |

### Layout

Twelve crates, ~38,000 lines. `CLAUDE.md` has the map and the conventions; the short version:

```
vex-text     rope, typed coordinates, transactions, anchors, selections
vex-buffer   Document: text + undo history + pending changes
vex-display  soft wrap, anchored viewport, folding, sticky scroll
vex-theme    OKLCH colour, five themes, type roles
vex-input    the Action vocabulary, modal grammar, keymap, macros, jumplist
vex-syntax   tree-sitter parsing on a worker, version-tagged
vex-render   wgpu quad + glyph pipelines, paint layers, offscreen capture
vex-project  gitignore-aware discovery, fuzzy ranking, ripgrep search, file watching
vex-lsp      JSON-RPC client, diagnostics, rename, formatting, code actions
vex-lua      init.lua: options, themes, bindings
vex-import   VSCode settings/themes/keys/snippets, .vsix, colour schemes
vex          the binary: chrome, executor, panes, settings, sessions
```

## Licence

MIT. The bundled fonts (IBM Plex, Instrument Serif) are SIL Open Font License 1.1 — see
`crates/vex-render/fonts/`. The logo is in `logo/`, with its usage rules.

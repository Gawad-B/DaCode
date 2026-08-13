# vex

A code editor. Rust, `winit` + `wgpu` + `glyphon` — no GUI framework, no web view.

Vim's grammar if you want it, a conventional editor if you don't. Tree-sitter highlighting, language
servers, splits, a file tree and a git gutter. One rule is enforced above the others: **nothing
unbounded ever runs on the UI thread**, which is why a keystroke costs the same in a 100,000-line file
as in a ten-line one.

`PLAN.md` has the architecture and the reasoning, including the decisions that turned out wrong.

**Status:** used daily by its author on Linux. Windows code paths are written and type-checked but the
editor has not been *run* there — see `packaging/windows/README.md`. No integrated terminal; see the
end of M7 in `PLAN.md` for why it is deferred rather than forgotten.

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
vex                      # nothing: reopens the last session for this directory
```

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

**Not a Vim user?** First row of the settings panel (`ctrl-,`), or `vim_keys = false`. Typing then
always inserts, the shortcuts are the familiar ones — `ctrl-c`, `ctrl-v`, `ctrl-x`, `ctrl-z`, `ctrl-y`,
`ctrl-a`, `ctrl-f`, shift-arrow to select — and the mouse places the cursor and selects by dragging.

**With Vim keys on**, the grammar composes as you expect: `3dw`, `d2f)`, `ci"`, `"ayy`, `.` to repeat.
Vertical motion follows *display* rows, so it moves through soft-wrapped lines the way they look.

Either way, these are vex's own and always work:

| Key | |
|---|---|
| `ctrl-p` · `ctrl-b` | file palette · buffer palette |
| `ctrl-e` | file tree |
| `ctrl-,` | settings |
| `ctrl-f` | search the project |
| `ctrl-k o` | open a folder |
| `ctrl-w v` · `ctrl-w w` | split · move between panes |
| `ctrl-s` · `ctrl-q` | save · quit |

## Settings

`ctrl-,` opens a panel with every option, its value, and — the part a file cannot tell you — **which of
default, `init.lua` or imported last set it**. `h`/`l` change a setting immediately; `w` writes the
panel's values to `init.lua`.

It also holds the VSCode import checklist and a **load a theme file…** row, so neither needs a
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

Language support is not extensions here: install `rust-analyzer`, `pyright-langserver`,
`typescript-language-server` and vex drives them over LSP directly.

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
cargo test --workspace                      # 512 tests
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
```

### Environment variables

| Variable | Effect |
|---|---|
| `VEX_CONFIG_DIR` | Configuration somewhere other than the platform default |
| `VEX_STATE_DIR` | Sessions and recovery elsewhere |
| `VEX_LSP_TRACE=1` | Log every message to and from the language server |
| `VEX_NO_SYSTEM_FONTS=1` | Bundled fonts only — faster startup, reproducible captures |

### Layout

Twelve crates, ~34,000 lines. `CLAUDE.md` has the map and the conventions; the short version:

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

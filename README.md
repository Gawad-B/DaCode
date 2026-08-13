# vex

A modal code editor. Rust, `winit` + `wgpu` + `glyphon`, no GUI framework and no web view.

The design goal, in the order it is enforced: **nothing unbounded on the UI thread**, Vim's grammar
with Helix's selection model, and VSCode's features earned one at a time rather than assumed. See
`PLAN.md` for the architecture and the decisions behind it — including the ones that turned out
wrong.

Status: usable for real editing, with gaps. No folding, no file tree yet. See the "what is left"
notes at the end of each milestone in `PLAN.md`.

## Build and run

```sh
cargo build --release
./target/release/vex path/to/file.rs   # or `vex` alone for an empty buffer
```

Debug builds are usable — `opt-level = 1` is set for exactly that reason — but the release binary is
what the performance claims refer to.

### Try it on this repository

```sh
./target/release/vex crates/vex-text/src/anchor.rs
```

If `rust-analyzer` is on `PATH` it starts automatically; the statusline says so, and diagnostics,
hover, go-to-definition, rename and formatting all become live. Without it everything else still
works.

## Keys

Vim's grammar, so counts, registers and operator-motion composition work as you expect: `3dw`,
`d2f)`, `ci"`, `"ayy`, `.` to repeat. What follows is what is bound beyond that.

### Motions
`h j k l` · `w W b B e E` · `0 ^ $` · `{ }` paragraphs · `%` matching bracket · `gg` `ge` file
start/end · `gh` `gl` line start/end · `G` with a count · `f F t T` find character

Vertical motion follows **display** rows, so it moves through soft-wrapped lines the way it looks.

### Operators and text objects
`d` delete · `c` change · `y` yank · `>` `<` indent · `U` uppercase · `L` lowercase — each applied to
a motion, to a doubled operator (`dd`, `yy`), or to a selection in visual mode.

Objects: `iw aw iW aW` · `i" i' i\`` · `i( i[ i{ i<` and their `a` forms · `ip ap` · `il al`

Structural objects (`if`, `ic` — function, class) are **not** bound yet, though the tree-sitter
machinery for them exists.

### Modes
`i a I A o O` insert · `v` visual · `R` replace · `esc` back to normal.
`esc` in normal mode collapses multiple cursors.

### Multiple cursors
`gA` above · `C` below · `S` one caret per selected line · `,` keep only the primary · `ga` select all
· `V` select line · `x` delete character

### Search and commands
`/` `?` search · `n` `N` next/previous · `ctrl-f` project search for the word under the cursor
`:` command line — `:w` `:w path` `:q` `:q!` `:wq` `:42` `:e path` `:grep pattern`
`:set wrap` / `:set nowrap` · `:format` · `:rename [name]`

### Files, buffers and panes
`ctrl-p` file palette · `ctrl-b` buffer palette · `ctrl-s` save · `ctrl-q` quit
`ctrl-w v` split beside · `ctrl-w s` split below · `ctrl-w c` close pane · `ctrl-w w` next pane

### Language server
`K` hover · `gd` go to definition · `gr` rename · `gf` format · `]d` `[d` next/previous diagnostic
`ctrl-space` completion (also in insert mode) · `esc` dismisses an info box

### Appearance
`z` toggle soft wrap · `gt` toggle light/dark · `=` `-` font size

## Configuration

Optional. vex starts with no configuration at all, and the built-in defaults are the real defaults.

```sh
mkdir -p ~/.config/vex
cp examples/init.lua ~/.config/vex/init.lua
```

`examples/init.lua` documents every option and shows how to bind keys. Mistakes are reported in a
box when the editor opens — with a did-you-mean for a misspelled action or theme token — and never
prevent startup.

To try a config without touching your own:

```sh
VEX_CONFIG_DIR=/tmp/vex-config ./target/release/vex
```

### Importing from VSCode

```sh
./target/release/vex import --help                        # the full list of sources and items
./target/release/vex import --from-vscode --list           # what your VSCode install offers
./target/release/vex import --from-vscode --settings --theme --keybindings --snippets
./target/release/vex import --openvsx zhuangtongfa.material-theme --theme
```

Opt-in per item: nothing is imported unless you name it, and `--list` imports nothing at all. Extensions are **not** run — see §5a of
`PLAN.md` for what is portable and what is not, and why.

## Verifying it without a window

The whole appearance can be rendered offscreen to a PNG, which is how the design is checked in CI
and over SSH — and how most of the visual defects in this project were actually found.

```sh
./target/release/vex --screenshot /tmp/vex.png                     # the editor
./target/release/vex --screenshot /tmp/vex.png --light             # light theme
./target/release/vex --screenshot /tmp/vex.png --mode insert       # any mode
./target/release/vex --screenshot /tmp/vex.png --palette an        # command palette
./target/release/vex --screenshot /tmp/vex.png --search Anchor     # search prompt
./target/release/vex --screenshot /tmp/vex.png --grep Anchor       # project search panel
./target/release/vex --screenshot /tmp/vex.png --complete ch       # completion popup
./target/release/vex --screenshot /tmp/vex.png --diagnostic 12     # a diagnostic message
./target/release/vex --screenshot /tmp/vex.png --splash            # cold-start lockup
./target/release/vex --screenshot /tmp/vex.png --empty             # empty state
./target/release/vex --screenshot /tmp/vex.png --split 2           # two panes side by side
./target/release/vex --screenshot /tmp/vex.png --split 3h          # three stacked
```

## Tests and benchmarks

```sh
cargo test --workspace          # 371 tests
cargo clippy --workspace --all-targets
cargo bench -p vex -p vex-syntax
```

The suite includes live language-server tests, which are **skipped** when `rust-analyzer` is not
installed rather than failing — a test that fails on a machine without a dependency is testing the
machine.

Useful when something misbehaves:

| Variable | Effect |
|---|---|
| `VEX_LSP_TRACE=1` | Logs every message to and from the language server |
| `VEX_NO_SYSTEM_FONTS=1` | Uses only the bundled fonts — faster startup, and reproducible captures |
| `VEX_CONFIG_DIR=<dir>` | Reads configuration from somewhere other than `~/.config/vex` |

## Installing

```sh
cargo build --release
packaging/linux/install.sh              # into ~/.local
packaging/linux/install.sh /usr/local   # or system-wide
```

Installs the binary, the desktop entry and the icon theme. See `packaging/README.md`.

The entry is written with an **absolute** `Exec` path rather than the bare `vex` the template carries.
A per-user install lands in `~/.local/bin`, which an interactive shell has on its `PATH` but a desktop
session routinely does not — so a launcher-clicked `Exec=vex` fails while the identical command works
in a terminal.

If the entry does not appear, GNOME appends newly installed apps to the **end** of the app grid rather
than sorting them in, so searching (`Super`, then type `vex`) finds it when scrolling does not.

## Recovering unsaved work

Dirty buffers are written to `$XDG_STATE_HOME/vex/recovery` every two seconds and forced before
quitting, so a crash costs seconds rather than work. Reopening the file offers the copy back. A copy
older than the file on disk is discarded rather than offered.

## Licence

MIT. The bundled fonts (IBM Plex, Instrument Serif) are SIL Open Font License 1.1 — see
`crates/vex-render/fonts/`.

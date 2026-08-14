# CLAUDE.md

Working notes for this codebase. `README.md` is for people using vex; this is for whoever is changing
it. `PLAN.md` is the long-form architecture and decision record — when this file and `PLAN.md`
disagree, `PLAN.md` is the reasoning and this is the summary.

## What this is

A code editor in Rust: `winit` + `wgpu` + `glyphon`, no GUI framework. Twelve crates, ~34,000 lines,
512 tests, clippy clean.

## Commands

```sh
cargo test --workspace
cargo clippy --workspace --all-targets     # must stay at zero warnings
cargo fmt --all
cargo build --release

cargo run -p vex-input --example cheatsheet   # CHEATSHEET.md vs the real grammar
cargo bench -p vex -p vex-syntax
cargo check --workspace --target x86_64-pc-windows-msvc   # 7 of 9 crates; see "Windows" below
```

`VEX_NO_SYSTEM_FONTS=1` makes captures reproducible and startup faster. Use it for every screenshot.

## The one rule

**Nothing unbounded may run on the UI thread.** Everything else in the design follows from it:

- Background work gets a cheap rope snapshot plus the document's `Version`, and returns results
  **tagged with the version it saw**. The UI applies a result only if that version still matches;
  otherwise it is discarded. Syntax, LSP, git diff and the file walk all follow this.
- Anything per-frame must be bounded by the **viewport**, not the document. The one real performance
  defect found so far was a sidebar walking 512 display rows to compute one coordinate: bounding it to
  the ~40 visible rows took frame build from 1.13 ms to 84 µs.
- Keystroke cost is flat with document size: ~7 µs at 1k lines and at 100k.

If you add a background producer, give it a `Version` and a `poll()` that never blocks, and wire it
into `about_to_wait` beside the others.

## Architecture, in dependency order

```
vex-text     rope (ropey), typed coordinates, Edit/Transaction, Anchor, SelectionSet
vex-theme    Oklch, five themes, type roles, mode accents
vex-display  soft wrap, ScrollAnchor, Folds, sticky scroll
vex-buffer   Document = text + undo history + pending changes + pairing
vex-input    Action, Grammar, Keymap, JumpList, Macros
vex-syntax   tree-sitter on a worker thread
vex-render   Frame -> quads/glyphs/marks, paint Layers, Headless capture
vex-project  FileIndex, FileWalk, ProjectSearch, FileWatcher
vex-lsp      transport, client, Diagnostics
vex-lua      init.lua -> Config
vex-import   VSCode data, .vsix, colour schemes in four formats
vex          the binary: chrome, execute, panes, layout, settings, session, welcome
```

### Non-obvious invariants

**Resolve paths through `vex_project::canonical`, never `canonicalize` directly.** On Windows the
latter returns `\\?\C:\x`, which is correct for the filesystem API and wrong for displaying,
comparing and URI-building. Two separate CI failures came from this one difference: a URI a language
server refused, and a root compared against `temp_dir()`. There are now zero bare `canonicalize()`
calls in `crates/vex`; keep it that way.

**URIs are not paths.** LSP wants `file:///C:/x/y.rs` on Windows — three slashes, forward separators,
no `\\?\` prefix from `canonicalize`. `file://C:/x` names a *host* called `C:`. Go through
`position::encode_uri` / `decode_uri`, never build one by hand.

**Typed coordinates.** `ByteOffset`, `CharOffset`, `Utf16Column`, `Point`, `DisplayPoint` are distinct
types and never interchangeable. LSP speaks UTF-16; the rope speaks chars; the screen speaks display
rows. Bare integers for positions are how editors get their worst bugs — do not add one.

**One mutation path.** Every text change goes through `Document::apply(Transaction, EditKind)`, which
returns the exact inverse for undo. Nothing else may touch the rope. `EditKind::Discrete` means "its
own undo group" — use it for anything that is one *intent* however many edits it makes (a rename, a
format, a macro replay, a snippet expansion).

**Everything is an `Action`.** Keys resolve to one, the palette lists them, macros record them, config
binds them, tests drive them. That is why macros survive rebinding, and why `3dw` can be tested
headlessly. Adding a user-visible operation means adding an `Action`, a name in `Action::from_name`,
that name in `Action::NAMES`, and a row in `vex_input::commands::COMMANDS` — the name is public API
that config depends on, and three tests fail until all four exist.

**`commands::COMMANDS` is the only description of what the keys are.** The command palette, the help
panel (F1), the menu bar and the which-key popup all read that one table, and a test feeds every
non-modal key in it through the real grammar. Four features that each maintained their own list would
disagree within a month, and a discoverability feature that lies is worse than none — it teaches a key
that does nothing. If you add a binding to `feed_flat`, add its row, or the table is wrong.

**The non-modal mode is a first-class editor, not the modal one with the modes removed.** `feed_flat`
carries the whole conventional vocabulary — line moves, duplication, comment toggle, occurrence
selection, word deletion — because Vim reaches those by *composition* (`ddp`, `yyp`) and with the
grammar off there is nothing to compose with. It had thirteen keys once and that is exactly what it
felt like.

**The executor owns no processes.** `execute()` records a request on the state (`pending_request`) or
returns `Outcome::Window(action)`; the event loop, which owns the LSP client and the pane list, carries
it out. That split is what lets hover and rename be tested with no server running.

**Paint layers.** The renderer draws all quads then all glyphs *per layer*, so an overlay on a higher
layer occludes text below it. Within one layer it cannot. Panels go on `Layer::Panel`, the palette on
`Layer::Modal`. Forgetting this is how the completion popup shipped transparent.

**Layout is data.** `crates/vex/src/layout.rs` is the single definition of where things are; both
drawing and mouse hit-testing read it. Two copies of that arithmetic drift, and the symptom — clicks
landing a line off — gets blamed on the display map.

**A theme name is a file *or* a built-in.** `config::resolve_theme` tries `themes/<slug>.toml` and then
`Theme::by_name`, in that order. It only did the first, which meant the settings panel — which writes
`theme = "amoled"` — produced config its own reader rejected on the next start.

**Folds live in the display map** because folding *is* a mapping from buffer lines to display rows.
They are cleared on buffer change: they are line numbers and nothing more.

## Testing conventions

- Test names are sentences about behaviour: `a_stale_reply_is_refused_whole`, not `test_apply_2`.
- Every test says *why* the behaviour matters when it is not obvious, in a comment. A test that only
  asserts is a test nobody can safely change.
- Property tests (`proptest`) for anything about coordinates or text invariants.
- Live `rust-analyzer` tests **skip** when it is absent rather than failing. A test that fails on a
  machine without a dependency is testing the machine. Skip on `vex_lsp::can_start`, not
  `is_installed`: on any rustup machine `rust-analyzer` is a *shim* that exists whether or not the
  component does, and speaking LSP to it fails in a way that looks like a vex bug. CI failed on exactly
  this.
- When a test asserts against a list of allowed values, adding a new value means updating the list. The
  Windows CI run's only failure was `describe()` returning `"windows shell"`, which the test's list did
  not include — a value that cannot occur on the machine the test was written on.
- **A rule that keeps being broken becomes a test.** `crates/vex/tests/conventions.rs` greps the
  workspace for direct `canonicalize()` calls, because that one slip caused three consecutive Windows CI
  failures and each cost a fifteen-minute round trip to find. Verify such a guard by *injecting a
  violation* — the first version scanned zero files (cargo runs tests with the crate as the working
  directory, not the workspace) and passed while checking nothing.
- **Never mutate process-wide state in a test.** Environment variables in particular: `setenv` is not
  thread-safe against the `getenv` other tests do in parallel, and doing it deadlocked this suite
  inside glibc for twenty minutes. Take the value as a parameter instead — see
  `paths::resolve(Where, bool, &Environment)`, which also means the *Windows* rules are testable from
  Linux. The recovery and session directories are parameters for the same reason.

### Verify visual work by rendering it

`--screenshot out.png` renders any state offscreen and exits. Read the PNG. This has caught, among
others: a transparent completion popup, tofu boxes where the bundled font had no glyph, a logo that
came out as an X, clipped diagnostic messages, and a lockup misaligned by 2px. Screen capture does not
work on Wayland here, so this is the only way — and it is better anyway, being reproducible.

Flags: `--light --dark --theme NAME --mode M --split N[h] --tree --settings --welcome STEP --folded
--git --palette Q --search Q --grep Q --complete PREFIX --diagnostic L:C:MSG --diagnostics-list
--splash --empty --at LINE --columns N --no-vim --help-panel --menu --context-menu --commands
--which-key PREFIX`.

## Style

- Comments explain **why**, never what. If a line's purpose is obvious, it gets no comment.
- Prefer the honest limitation over the clever workaround, and *say* the limitation in the doc comment.
  Several of the best comments in this codebase record something that went wrong.
- Commit messages are prose, not bullet lists of files. Say what changed, why, and what was rejected.
- British-ish spelling in prose is fine; identifiers are American (`color` in code, "colour" in text) —
  because the wgpu and CSS vocabulary around them is.

## Windows

The workspace **builds and its tests pass on Windows** in CI, including a live rust-analyzer handshake.
Nobody has *run* the editor there — the window, mouse and folder chooser are unverified. When touching
anything platform-shaped:

- Paths go through `crates/vex/src/paths.rs`. `%APPDATA%` for config (roams), `%LOCALAPPDATA%` for
  state (must not roam).
- Program lookup must consult `PATHEXT` — a bare-name check finds neither `rust-analyzer.exe` nor the
  `.cmd` shims most language servers install as.
- Write Windows-only functions **without** `#[cfg(windows)]` and select them with a runtime
  `cfg!(windows)`. A `#[cfg(windows)]` body is never type-checked on Linux, and that is exactly the
  code that cannot be tested here.
- Only 7 of 9 crates type-check for the MSVC target from Linux; `vex-syntax` and `vex-lua` need a
  Windows C toolchain. `.github/workflows/build.yml` has a Windows job for that reason — and it earned
  its keep immediately: it caught `file://` URIs being built by percent-encoding a whole Windows path,
  which made rust-analyzer reject every request as an "invalid international domain name" and broke
  every language-server feature on Windows. Nothing on Linux could have seen it.
- **Spawn through `vex::process::command`, never `Command::new`.** A release build is a GUI
  application with no console, so Windows gives every console child — git, powershell, a language
  server — a brand new console window, in front of the editor. `CREATE_NO_WINDOW` is what suppresses
  it. `vex-lsp` keeps its own copy of the helper because it sits below the binary; `crates/vex/tests/
  conventions.rs` enforces both.
- **Anything platform-shaped gets the platform as a parameter**, like `paths::resolve` and
  `position::encode_uri(path, windows)`. Then the Windows rules have tests that run here. A
  `cfg!(windows)` branch inside the function is untestable from Linux, which is how the URI bug
  survived to CI.

## Where things are

| Want to change | Look at |
|---|---|
| A key binding | `crates/vex-input/src/grammar.rs`, then `commands.rs`, `CHEATSHEET.md` + the example |
| What the palette, help and menus say | `crates/vex-input/src/commands.rs` — one table, four readers |
| What a command does | `crates/vex/src/execute.rs` |
| How something looks | `crates/vex/src/chrome.rs` |
| Where something is | `crates/vex/src/layout.rs` |
| A config option | `vex-lua/src/lib.rs`, `vex/src/config.rs`, `vex/src/settings_ui.rs` (all three) |
| A theme | `crates/vex-theme/src/theme.rs` |
| LSP behaviour | `crates/vex-lsp/src/client.rs`, applied in `crates/vex/src/edits.rs` |

Adding a config option touches three places by design: the Lua reader, the settings struct, and the
panel. If you add it to fewer, it will silently not work somewhere.

## Known gaps

Deliberate, with reasons in `PLAN.md`: no integrated terminal, no git blame, no structural text objects
(`if`/`ic`), tree-sitter indent queries unused, column advance derived from the type scale rather than
measured from the shaper, no macOS packaging. Cross-file rename is not undoable and says so.

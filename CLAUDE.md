# CLAUDE.md

Working notes for this codebase. `README.md` is for people using vex; this is for whoever is changing
it. `PLAN.md` is the long-form architecture and decision record — when this file and `PLAN.md`
disagree, `PLAN.md` is the reasoning and this is the summary.

## What this is

A code editor in Rust: `winit` + `wgpu` + `glyphon`, no GUI framework. Twelve crates, ~38,000 lines,
616 tests, clippy clean.

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
vex-input    Action, Grammar, Keymap, JumpList, Macros, the command table
vex-syntax   tree-sitter on a worker thread
vex-render   Frame -> quads/glyphs/marks, paint Layers, Headless capture
vex-project  FileIndex, FileWalk, ProjectSearch, FileWatcher
vex-lsp      transport, client, Diagnostics
vex-lua      init.lua -> Config
vex-import   VSCode data, .vsix, colour schemes in four formats
vex          the binary: chrome, execute, panes, layout, settings, session, welcome,
             palette, help, menu, clipboard, process
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

**A character with ctrl or alt held is a *binding*, not text.** Window systems apply shift before the
key reaches us, so `ctrl-shift-p` arrives as `P` — and every binding written as `Char('p') if shift`
silently never matched. That shipped: the command palette, delete-line, select-all-occurrences, redo
and project search were all dead in 0.2.0. `Key::from_platform` lowercases the character when a
modifier is held and leaves `shift` on the key for bindings that mean it. Never build a `Key` from a
platform event any other way.

**Every mouse-reachable thing is geometry data plus a click handler that reads it.** `layout.rs` for
the chrome and the toolbar, `menu::geometry`, `welcome::geometry`. The drawing code reads the same
function. A rectangle computed twice is a click that lands somewhere other than where it looks.

**If a feature can only be reached by a key, it is not finished.** The toolbar, the menus, the context
menu, the clickable settings rows and the clickable wizard all exist because someone who chose the
conventional editor was handed a keyboard-only interface — including on the wizard screen *asking*
whether they wanted the mouse-shaped editor. Every one of those surfaces prints the shortcut for what
it does, so the mouse path teaches the keyboard rather than replacing it.

**The non-modal mode is a first-class editor, not the modal one with the modes removed.** `feed_flat`
carries the whole conventional vocabulary — line moves, duplication, comment toggle, occurrence
selection, word deletion — because Vim reaches those by *composition* (`ddp`, `yyp`) and with the
grammar off there is nothing to compose with. It had thirteen keys once and that is exactly what it
felt like.

**The executor owns no processes.** `execute()` records a request on the state (`pending_request`) or
returns `Outcome::Window(action)`; the event loop, which owns the LSP client and the pane list, carries
it out. That split is what lets hover and rename be tested with no server running.

**Never configure the surface while holding a frame.** wgpu requires the acquired `SurfaceOutput` to
be dropped *before* `Surface::configure`. Doing both at once is a validation error, which the default
handler turns into a panic, whose unwind drops the texture, whose drop is a second validation error,
which panics again — and a panic during a panic aborts. That was the whole of "minimise vex and it
closes": minimising is what makes the surface start reporting `Suboptimal`, and that branch
reconfigured inline. `Renderer` now sets a `reconfigure` flag and acts on it at the top of the *next*
frame. The device also has an uncaptured-error handler that reports instead of panicking, because a
GPU error must never take unsaved work with it — a release build on Windows has no console to say so
in.

**Nothing draws without being measured against its box.** `run()`/`label()` set `wrap: false` and
glyphon clips a run to its rectangle, so text that is too long simply stops mid-word with nothing to
say it was cut. Anything user-supplied or variable-length goes through `chrome::elide`, which uses
`Typography::advance` — a type-scale estimate, not the shaper, so it can be a character out either
way. Better an approximate ellipsis than an exact guillotine.

**The horizontal scroll offset enters pixel arithmetic only through `layout::CodeGrid`.** That type
is the single mapping between a display column and an x, read by `draw_code` to place every glyph and
by `App::offset_at` to turn a click into an offset. There were two copies before it existed and they
already disagreed about the left pad — drawing used `space::SM` (12) and the mouse subtracted 8 — so
a click landed most of a character out. A second copy plus an offset is a click landing in a
different column from the one under the pointer, growing as you scroll. Text is *sliced* to the
visible columns rather than drawn at a negative x: a run is clipped to its own rect, which is the one
starting at that negative x, and glyphs are drawn after all quads in a layer — so scrolled code would
paint over the line numbers. Quads positioned from a column go through `Rect::intersect(code)` for
the same reason; `Frame::fill` clips nothing.

**A panel that scrolls must hand its rectangles to the mouse.** `settings_ui::panel` returns the
visible rows and their boxes; drawing and hit-testing both read it. Teaching the drawing half to
scroll while the click handler kept its own copy of the walk immediately produced clicks landing on
the row that *used* to be there — the same failure `layout.rs` exists to prevent, one panel over.

**Resolve a program to a path before spawning it.** `vex_lsp::resolve` returns the `PathBuf`, not a
yes/no, because on Windows the two are different questions: the check honours `PATHEXT` and finds
`pyright-langserver.cmd`, while `Command::new` on a bare name only ever appends `.exe`. They
disagreed, so no npm-installed language server could start at all. Hand the resolved path to
`Command::new` and std routes a `.cmd` through `cmd.exe` itself with the escaping CVE-2024-24576
added; hand-rolling `cmd /c` would reopen that hole.

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
- **A test that names a program must name one the *platform* guarantees**, not one the machine happens
  to have. `picker.rs` asserted `which("sh")` and passed everywhere it had ever run — including the
  Windows runner, whose image ships Git's `sh.exe` — then failed on the first real Windows developer
  machine, which does not have one. `cmd` on Windows, `sh` elsewhere, as `paths.rs` already did.
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
--splash --empty --at LINE --at-column N --columns N --no-wrap --size WxH --no-vim --help-panel
--menu --context-menu --commands --which-key PREFIX`.

**Render at more than one size.** `--size WxH` exists because the harness was fixed at 1600x1000,
and a whole family of defects lives below that: the settings rows collided with the footer under
~900px of height, the toolbar was laid out at a negative x under ~450px of width, and neither was
reproducible here. Four overflow defects shipped for exactly this reason. Anything with a panel in
it gets captured at a small size too.

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
| What the toolbar offers | `crates/vex/src/layout.rs` — `TOOLBAR`, read by drawing and by the mouse |
| What a right-click offers | `crates/vex/src/menu.rs` — `CONTEXT`, names from the command table |
| A theme | `crates/vex-theme/src/theme.rs` |
| LSP behaviour | `crates/vex-lsp/src/client.rs`, applied in `crates/vex/src/edits.rs` |
| Which language server runs | `crates/vex-lsp/src/lib.rs` — `servers_for`, a list per language |
| Where the settings rows are | `crates/vex/src/settings_ui.rs` — `panel`, read by drawing and by the mouse |
| Whether a file may be opened | `crates/vex/src/open.rs` — every refusal is a sentence for the screen |

Adding a config option touches three places by design: the Lua reader, the settings struct, and the
panel. If you add it to fewer, it will silently not work somewhere.

## Known gaps

Deliberate, with reasons in `PLAN.md`: no integrated terminal, no git blame, no structural text objects
(`if`/`ic`), tree-sitter indent queries unused, column advance derived from the type scale rather than
measured from the shaper, no macOS packaging. Cross-file rename is not undoable and says so.

Smaller ones, from the non-modal work: a drag after a double-click extends by character rather than by
word; the which-key popup sits at the top left of the code area rather than following the cursor,
because a popup that moved as you typed would be harder to read than a still one; and the palette and
help panel take the keyboard only — their rows are not clickable, though the menus' are.

Known and stated rather than hidden: **buffer search is still linear in the document**. `find_all`
no longer copies the rope — it scans a line at a time, so the memory is bounded and a query
containing a newline falls back to the whole-document path — but a rare query in a very large file
still walks all of it on the UI thread. The fix is the same shape as syntax and git: a worker with a
`Version` tag. **Opening a file is still synchronous** for the same reason; what changed is that it
is now *bounded* (`open::MAX_BYTES`) and that binary files and non-UTF-8 files are refused with a
sentence rather than mangled. Neither is asynchronous yet, and the doc comments say so.

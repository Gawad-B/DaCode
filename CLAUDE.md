# CLAUDE.md

Working notes for this codebase. `README.md` is for people using DaCode; this is for whoever is changing
it. `PLAN.md` is the long-form architecture and decision record — when this file and `PLAN.md`
disagree, `PLAN.md` is the reasoning and this is the summary.

## What this is

A code editor in Rust: `winit` + `wgpu` + `glyphon`, no GUI framework. Twelve crates, ~50,000 lines,
744 tests, clippy clean.

## Commands

```sh
cargo test --workspace
cargo clippy --workspace --all-targets     # must stay at zero warnings
cargo fmt --all
cargo build --release

cargo run -p dacode-input --example cheatsheet   # CHEATSHEET.md vs the real grammar
cargo bench -p dacode -p dacode-syntax
cargo check --workspace --target x86_64-pc-windows-msvc   # not all of them; see "Windows" below
```

`DACODE_NO_SYSTEM_FONTS=1` makes captures reproducible and startup faster. Use it for every screenshot.

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

**`crates/dacode/tests/latency.rs` is the rule as a test that fails the build.** It asserts *flatness*,
not milliseconds: a hundredfold larger document may cost at most 3x more, which is machine-
independent in a way an absolute budget is not — a loaded runner is slower at everything, so any
threshold tight enough to catch a regression is also tight enough to fail at random. Absolute
ceilings are there too, set where only a catastrophe trips them. Verified the way every guard here
must be, by injecting a violation: a document-length walk added to `chrome::draw` fails it at 104x,
and one added to `InsertText` fails it at 111x. `cargo bench` still measures; this is what *fails*.

## Architecture, in dependency order

```
dacode-text     rope (ropey), typed coordinates, Edit/Transaction, Anchor, SelectionSet
dacode-theme    Oklch, six themes, type roles (one face: Fira Code), mode accents
dacode-display  soft wrap, ScrollAnchor, Folds, sticky scroll
dacode-buffer   Document = text + undo history + pending changes + pairing
dacode-input    Action, Grammar, Keymap, JumpList, Macros, the command table
dacode-syntax   tree-sitter on a worker thread
dacode-render   Frame -> quads/glyphs/marks, paint Layers, Headless capture
dacode-project  FileIndex, FileWalk, ProjectSearch, FileWatcher
dacode-lsp      transport, client, Diagnostics
dacode-lua      init.lua -> Config
dacode-import   VSCode data, .vsix, colour schemes in four formats
dacode          the binary: chrome, execute, panes, layout, settings, session, welcome,
             palette, help, menu, clipboard, process
```

### Non-obvious invariants

**Resolve paths through `dacode_project::canonical`, never `canonicalize` directly.** On Windows the
latter returns `\\?\C:\x`, which is correct for the filesystem API and wrong for displaying,
comparing and URI-building. Two separate CI failures came from this one difference: a URI a language
server refused, and a root compared against `temp_dir()`. There are now zero bare `canonicalize()`
calls in `crates/dacode`; keep it that way.

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

**In the binary, that path is `EditorState::mutate`, not `Document::apply`.** The document is the
funnel for changing text, but folds live *outside* it in the display map and are line numbers —
nothing moved them, so deleting a function above a closed fold left a range running off the end of
the file, and since `is_hidden` only asks whether a line falls inside the range, that hid everything
below the fold's head. `mutate` takes a closure, runs it against the document, then replays
`changes_since` over the folds through `dacode_text::transform_offset`. `conventions.rs` greps for it,
because "remember to call the other thing too" is not a rule that survives a year — and that grep
found two sites hand-editing had already missed.

**Nothing outside the document may hold a bare line number *or offset* across an edit.** Folds were
the case that existed. The highlight spans were the second and nobody had noticed: they are only ever
*set* from a parse whose version matched the buffer, which was read as meaning they could never be
stale — but the next keystroke makes them stale, and a parse takes milliseconds to answer, so every
span after the cursor named text that had already moved. Typing a hundred characters into a line
rendered the rest of the file in the colours of the text a hundred characters earlier: comment italics
running through code, the wrong font on half a line. It corrected itself the moment typing stopped,
which is why it read as a rendering fault rather than a stale-data one. `mutate` moves them now.

The diagnostics were the third, and the longest-lived: a parse answers in milliseconds, while a
diagnostic is a round trip to another process, so its underlines sat under the wrong code for as long
as the server took. They are moved in `mutate` too, but **the arithmetic is done in their own
line-and-column coordinates**, and that is the whole design rather than an optimisation. Converting
each range to an offset and back is two rope lookups per diagnostic: a keystroke in a
hundred-thousand-line file carrying ten thousand of them cost 39.6 ms, more than two frames, for one
character. Converting each *edit* once and moving every diagnostic with integer arithmetic costs
about a tenth of a millisecond for the same keystroke. `latency.rs` asserts both halves — the same
pile of diagnostics costs the same in a large file as in a small one, and a pathological pile stays
inside a frame — and a return to the rope version fails it.

The search matches are the fourth and are **deliberately left drifting**: they cover the file where
the spans cover the viewport, so moving them per keystroke measured 24.6x on a hundred-thousand-line
document (311 µs against 13 µs) and `latency.rs` holds that measurement to refuse the one-line
version of the repair. Doing it properly means moving them lazily — at the viewport for drawing, once
per `n` for stepping. If you add a fifth, transform it in `mutate` beside the spans or clear it, and
say which.

**Everything is an `Action`.** Keys resolve to one, the palette lists them, macros record them, config
binds them, tests drive them. That is why macros survive rebinding, and why `3dw` can be tested
headlessly. Adding a user-visible operation means adding an `Action`, a name in `Action::from_name`,
that name in `Action::NAMES`, and a row in `dacode_input::commands::COMMANDS` — the name is public API
that config depends on, and three tests fail until all four exist.

**`commands::COMMANDS` is the only description of what the keys are.** The command palette, the help
panel (F1), the menu bar and the which-key popup all read that one table, and a test feeds every
non-modal key in it through the real grammar. Four features that each maintained their own list would
disagree within a month, and a discoverability feature that lies is worse than none — it teaches a key
that does nothing. If you add a binding to `feed_flat`, add its row, or the table is wrong.

**The command line is a table too, and it is checked against the source in three directions.**
`main.rs`'s `FLAGS` is what `--help` renders and what `docs/reference/cli.md` quotes. `VALUE_FLAGS`
beside it is the set whose *next argument* is a value, and it has now been wrong twice: first
`--columns 60` opened a file named `60`, and after that was fixed `--row`, `--symbols` and `--block`
were each added without being listed, so `--row 12` opened an empty buffer called `12` behind the
settings panel it was meant to be scrolling. Nothing failed and the panel still drew, which is why
rendering every overlay in CI could not see it. `command_line_tests` derives all of it from
`include_str!("main.rs")`: every flag whose value is consumed is in `VALUE_FLAGS`, every flag `FLAGS`
offers is really read, every flag really read is in `FLAGS`, and a flag listed for ordinary use is
read *outside* `screenshot` — which is the one that caught `--no-vim` being advertised as a way to
start the editor without Vim keys when it only ever worked with `--screenshot`. The scan strips
comment lines and matches only the three shapes a flag is read in, because its first version matched
the prose above the code and failed against a doc comment. A fifth test keeps the scan from passing
vacuously.

**A character with ctrl or alt held is a *binding*, not text.** Window systems apply shift before the
key reaches us, so `ctrl-shift-p` arrives as `P` — and every binding written as `Char('p') if shift`
silently never matched. That shipped: the command palette, delete-line, select-all-occurrences, redo
and project search were all dead in 0.2.0. `Key::from_platform` lowercases the character when a
modifier is held and leaves `shift` on the key for bindings that mean it. Never build a `Key` from a
platform event any other way.

**Every mouse-reachable thing is geometry data plus a click handler that reads it.** `layout.rs` for
the chrome's bars and buttons, `menu::geometry`, `welcome::geometry`, `settings_ui::panel`,
`tree::panel`, `layout::CodeArea`. The drawing code reads the same function. A rectangle computed
twice is a click that lands somewhere other than where it looks.

That rule has now been broken three times and each break looked like a different bug. `draw_code`
shadowed its own `code` parameter with what sticky scroll left of it, so the mouse, the hover box,
the completion popup and the which-key hint all measured against a rectangle a row taller per pinned
head — clicks landed up to three lines low, and because nothing is pinned at the top of a file it
read as intermittent. The file tree's click handler left no bottom pad where the drawing did, so the
two asked `FileTree::visible` for different heights and clicking one file opened another. **A
function that returns rows must return their rectangles too**, and the caller must do the splitting
before either half runs — `CodeArea::new` exists so `draw_code` cannot shrink a rectangle privately.

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
which panics again — and a panic during a panic aborts. That was the whole of "minimise DaCode and it
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

**Resolve a program to a path before spawning it.** `dacode_lsp::resolve` returns the `PathBuf`, not a
yes/no, because on Windows the two are different questions: the check honours `PATHEXT` and finds
`pyright-langserver.cmd`, while `Command::new` on a bare name only ever appends `.exe`. They
disagreed, so no npm-installed language server could start at all. Hand the resolved path to
`Command::new` and std routes a `.cmd` through `cmd.exe` itself with the escaping CVE-2024-24576
added; hand-rolling `cmd /c` would reopen that hole.

**A configured command containing a separator is a path, not a name.** `resolve_command` exists
beside `resolve` for that one distinction: `PATH` is a list of directories, so looking `.venv/bin/
pylsp` up in it finds nothing, ever — and a venv's own server is the main reason to name one.
Relative paths resolve against the **project root**, not the process's working directory, which for
a windowed editor is wherever the desktop launched it from and never what the user meant.

**A path typed into a prompt is relative to the project, never to the process.** The same reasoning,
and it caught Save As and `:w path` too: both stored the raw string, and `fs::write` resolves it
against whatever directory the desktop launched the editor from, so `src/notes.rs` wrote a file the
user could not find. Go through `absolute_in`. An absolute path is left alone — saving a copy outside
the project is a real thing to want.

**Containment is decided lexically, not by the filesystem.** `dacode_project::within` folds `.` and `..`
itself and refuses a climb above the root. Asking `canonicalize` cannot work for a file that does not
exist yet, which is the normal case for one being created: it fails with `NotFound`, `canonical`
falls back to returning its input with the `..` chain intact, and `Path::starts_with` compares
components — which normalise `.` and never `..`. So `x/../../../../evil.txt` passed the check and
`create_dir_all` walked it happily. It takes the root as a parameter, which is why it has tests;
nothing on Windows could have found this, because Win32 collapses `..` before the syscall.

**Buffer identity is the canonical path; buffer *order* is one function.** `buffer_key` decides
whether two buffers are the same file — it used to be `relative_crumbs`, a display path truncated to
four segments, so one file opened two ways got two undo histories and two files deep in different
trees collided. `stored_index` is the only mapping from a buffer's number to its slot in
`App::buffers`, which skips the live one; it existed and was written out by hand at five other sites
anyway, and that is exactly how `switch_to` came to renumber the whole list on every non-adjacent
switch. Anything that removes a buffer must also tell `Panes::buffer_closed`, or a split pane keeps
an index that now names a different file.

**A file the user owns is written through `dacode::save::write`, and a buffer is saved by
`App::write_active` alone.** `fs::write` truncates and then fills, so a crash or a full disk between
the two left an empty file — and the recovery copy was deleted the moment the save returned.
`save::write` goes through a hidden temporary beside the file, fsyncs it, takes the original's mode
and owner, and renames it over; it follows symbolic links rather than replacing them, and writes in
place (still fsynced) for a hard-linked file or one whose owner it cannot keep. `ctrl-s` used to reach
a second save in the executor that skipped the recovery discard, the watcher and `format_on_save`;
`conventions.rs` now counts `mark_saved()` calls so a second writer fails the build. Before writing,
`write_active` compares the file against the `save::Stamp` recorded when it was opened, reloaded or
last saved: a change from outside refuses the save once and says so, and asking again overwrites.
Autosave never takes that second step. Every dirty buffer is also handed to a `recovery::Lifeline`
once per batch of events, which the panic hook flushes before the stack unwinds.

**Every `didOpen` owes a `didClose`.** `opened_document` records what the server was told, and
`start_language_server` closes it before the next one opens — which covers switching, closing,
renaming and deleting, because all four reach `after_buffer_change`. Without it a server accumulated
every document of a session, each frozen at the version last sent.

**Formatting has two answers, and the server is asked at the handshake which one applies.**
`documentFormattingProvider` from the `initialize` reply decides whether `Action::Format` goes to the
server or to `format::FormatWorker`. pyright says no, so a Python file formats through ruff or black;
a plain JavaScript project has no server at all, and `sync_lsp` returns before it would ever take the
request — which is why `route_format` runs at the *top* of that function rather than beside its five
callers. A formatter is given text on stdin and never a path: one handed the file would rewrite it
behind a dirty buffer.

**A capability DaCode does not declare is a feature the server silently withholds.** rust-analyzer will
not offer an out-of-scope name at all unless the client advertises `completionItem.resolveSupport`
for `additionalTextEdits` — computing an import for each of a hundred candidates up front is too
slow to be worth doing, so it simply does not. Auto-import was therefore not a parsing gap but a
handshake one, and nothing in the editor could see it: the list came back looking perfectly normal
with the one useful entry missing. `labelDetailsSupport` is the same shape — without it the server
folds the detail into the label and an item arrives named `HashMap(use std::collections::HashMap)`.
When a server "does not support" something, check what DaCode claimed before assuming it does not.

**The language table is pivoted around the language, not the program.** `dacode_lsp::LANGUAGES` carries
the extensions, the `languageId` and the servers together, because `dacode.server("python", …)` names a
*language* and matching that to a file needs the extension list — which used to exist only as arms
of a `match`, reachable by nothing. `ServerSpec` is owned rather than `&'static str` for the same
reason: a server named in `init.lua` is not static, and pretending otherwise costs more than the
allocation does. A configured entry goes **ahead** of the built-ins rather than replacing them, so
naming a venv does not also throw away the fallback.

**Paint layers.** The renderer draws all quads then all glyphs *per layer*, so an overlay on a higher
layer occludes text below it. Within one layer it cannot. Panels go on `Layer::Panel`, the palette on
`Layer::Modal`. Forgetting this is how the completion popup shipped transparent.

**A bound written in two places is a bound that disagrees.** The settings panel clamped `font_size`
to 8–32 and wrapped a click at 32; `Typography::with_code_size` clamps at 11–18, and `Values` is read
back from the live typography. So the panel's value could never reach 32, the wrap never fired, and a
mouse-only user who clicked the size up to 18 was stuck — a control that traps the person using it,
which is the exact thing `cycle` exists to prevent, and its doc comment said so while doing the
opposite. Three tests asserted 8 and 32 and passed throughout, because they drove a bare `Values`
rather than the editor's path. Take the limit from the type that enforces it.

**Layout is data.** `crates/dacode/src/layout.rs` is the single definition of where things are; both
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
  machine without a dependency is testing the machine. Skip on `dacode_lsp::can_start`, not
  `is_installed`: on any rustup machine `rust-analyzer` is a *shim* that exists whether or not the
  component does, and speaking LSP to it fails in a way that looks like a DaCode bug. CI failed on exactly
  this.
- When a test asserts against a list of allowed values, adding a new value means updating the list. The
  Windows CI run's only failure was `describe()` returning `"windows shell"`, which the test's list did
  not include — a value that cannot occur on the machine the test was written on.
- **A rule that keeps being broken becomes a test.** `crates/dacode/tests/conventions.rs` greps the
  workspace for direct `canonicalize()` calls, because that one slip caused three consecutive Windows CI
  failures and each cost a fifteen-minute round trip to find. It also greps for text changed outside
  `EditorState::mutate`, which is what carries the folds. Verify such a guard by *injecting a
  violation* — the first version scanned zero files (cargo runs tests with the crate as the working
  directory, not the workspace) and passed while checking nothing.
- **A guard that reads a *window* of lines can pass while checking nothing.** The mutation grep's
  first version looked a few lines back for a nearby `mutate(` call, and the injected violation
  happened to sit two lines under an unrelated one — green, and worthless. It checks the *receiver*
  now: inside the closure the document is named plainly, everywhere else it is reached through
  something. Prefer a test that can only be satisfied one way over one that looks for context.
- **A test must drive the real code path, not a copy of it.** The first version of the session
  ordering test wrote out its own restore loop, so it passed whatever `main` did — and passed with
  the defect injected. `restore_session` and `session_snapshot` exist because the logic was
  unreachable from a test while it lived inside `main` and inside the function that also wrote the
  file. If a defect is not reachable from a test, that is the first thing to fix.
- **A timing guard has to measure the thing, not the early return.** `median` reuses one
  `EditorState` across thousands of iterations, so a keystroke test for electric dedent measured the
  scan once and the "line is no longer blank" early return every time after. `latency.rs` calls
  `dedent_for_closer` directly for that reason. Injecting the unbounded walk back reports 96x.
- **Documentation is checked the same way code is.** `conventions.rs` resolves every relative link in
  every `.md` file, because a dead link costs nothing to write, nothing notices, and the person who
  meets it was looking for exactly the thing it promised. It found four on its first run. `dacode-lua`
  loads `examples/init.lua` through the real reader for the same reason: that file is what the
  documentation tells people to copy, and nothing read it until a test did — a renamed option or a
  mistyped action name would have been found by the first person to follow the instructions. Both
  guards assert they scanned something before asserting they found nothing wrong.
- **A test that names a program must name one the *platform* guarantees**, not one the machine happens
  to have. `picker.rs` asserted `which("sh")` and passed everywhere it had ever run — including the
  Windows runner, whose image ships Git's `sh.exe` — then failed on the first real Windows developer
  machine, which does not have one. `cmd` on Windows, `sh` elsewhere, as `paths.rs` already did.
- **The same goes for a path.** A test opening `sub/../a.rs` without creating `sub` passes on Windows
  and nowhere else: Win32 collapses `..` lexically before the syscall, and every other kernel walks
  it and returns `ENOENT`. That asymmetry is the reason `dacode_project::within` exists at all, and it
  still caught a test written an hour after the function was. Create every directory a test path
  names, even the ones it only passes through.
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
--splash --empty --at LINE --at-column N --row N --columns N --no-wrap --size WxH --no-vim
--help-panel --menu --context-menu --commands --which-key PREFIX --symbols QUERY
--block LINE:COLUMN`.

`--row N` selects a settings row, which is the only way to see a section below the fold — the panel
scrolls to its selection, and the language-server report is always below it.

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

- Paths go through `crates/dacode/src/paths.rs`. `%APPDATA%` for config (roams), `%LOCALAPPDATA%` for
  state (must not roam).
- Program lookup must consult `PATHEXT` — a bare-name check finds neither `rust-analyzer.exe` nor the
  `.cmd` shims most language servers install as.
- Write Windows-only functions **without** `#[cfg(windows)]` and select them with a runtime
  `cfg!(windows)`. A `#[cfg(windows)]` body is never type-checked on Linux, and that is exactly the
  code that cannot be tested here.
- Not every crate type-checks for the MSVC target from Linux; `dacode-syntax` and `dacode-lua` need a
  Windows C toolchain. `.github/workflows/build.yml` has a Windows job for that reason — and it earned
  its keep immediately: it caught `file://` URIs being built by percent-encoding a whole Windows path,
  which made rust-analyzer reject every request as an "invalid international domain name" and broke
  every language-server feature on Windows. Nothing on Linux could have seen it.
- **Spawn through `dacode::process::command`, never `Command::new`.** A release build is a GUI
  application with no console, so Windows gives every console child — git, powershell, a language
  server — a brand new console window, in front of the editor. `CREATE_NO_WINDOW` is what suppresses
  it. `dacode-lsp` keeps its own copy of the helper because it sits below the binary; `crates/dacode/tests/
  conventions.rs` enforces both.
- **Anything platform-shaped gets the platform as a parameter**, like `paths::resolve` and
  `position::encode_uri(path, windows)`. Then the Windows rules have tests that run here. A
  `cfg!(windows)` branch inside the function is untestable from Linux, which is how the URI bug
  survived to CI.

## Where things are

| Want to change | Look at |
|---|---|
| A key binding | `crates/dacode-input/src/grammar.rs`, then `commands.rs`, `CHEATSHEET.md` + the example |
| What the palette, help and menus say | `crates/dacode-input/src/commands.rs` — one table, four readers |
| What a command does | `crates/dacode/src/execute.rs` |
| A command-line flag | `crates/dacode/src/main.rs` — `FLAGS` and `VALUE_FLAGS`, both checked against the source |
| What the docs say | `docs/` — tutorial, guides, reference, explanation; `README.md` is the landing page |
| The web landing page | `web/` — Vite + React, no framework beyond it; `web/README.md` says what drives it |
| How something looks | `crates/dacode/src/chrome.rs` |
| Where something is | `crates/dacode/src/layout.rs` |
| A config option | `dacode-lua/src/lib.rs`, `dacode/src/config.rs`, `dacode/src/settings_ui.rs` (all three) |
| What the title, activity and tab bars offer | `crates/dacode/src/layout.rs` — `title_bar`, `ACTIVITIES`, `TAB_BAR_COMMANDS`, read by drawing and by the mouse |
| An icon | `crates/dacode-render/src/icon.rs` — geometry on a 24-unit grid, drawn by `shape.wgsl` |
| What a right-click offers | `crates/dacode/src/menu.rs` — `CONTEXT`, names from the command table |
| A theme | `crates/dacode-theme/src/theme.rs` |
| LSP behaviour | `crates/dacode-lsp/src/client.rs`, applied in `crates/dacode/src/edits.rs` |
| Which language server runs | `crates/dacode-lsp/src/lib.rs` — `LANGUAGES`, plus `dacode.server` in config |
| Where the settings rows are | `crates/dacode/src/settings_ui.rs` — `panel`, read by drawing and by the mouse |
| Where the tree's rows are | `crates/dacode/src/tree.rs` — `panel`, same rule |
| Whether a file may be opened | `crates/dacode/src/open.rs` — every refusal is a sentence for the screen |
| Whether a path is inside the project | `crates/dacode-project/src/lib.rs` — `within`, lexical and testable |
| Which buffer is which | `crates/dacode/src/main.rs` — `buffer_key` for identity, `stored_index` for order |
| What survives an edit | `crates/dacode/src/chrome.rs` — `EditorState::mutate` |
| How a file is written | `crates/dacode/src/save.rs` — `write`, and `App::write_active` for buffers |

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

Electric dedent gives up after `DEDENT_SCAN_LIMIT` characters, so a `}` whose `{` is more than four
or five hundred lines above it is left where it was typed. It used to walk to the start of the
document, which cost 5.8 ms per keystroke in a hundred-thousand-line file. Doing nothing is what
already happened when no opener was found, so there was nothing new to invent.

Only the *active* buffer gets a file watcher, a git diff, and a language server. Background buffers
get crash-recovery writes and nothing else, so a file changed on disk behind an unfocused buffer is
noticed when you switch to it rather than when it happens. Deliberate — the alternative is N watchers
and N servers — but it is a limitation, not an oversight.

Known and stated rather than hidden: **buffer search is still linear in the document**. `find_all`
no longer copies the rope — it scans a line at a time, so the memory is bounded and a query
containing a newline falls back to the whole-document path — but a rare query in a very large file
still walks all of it on the UI thread. The fix is the same shape as syntax and git: a worker with a
`Version` tag. **Opening a file is still synchronous** for the same reason; what changed is that it
is now *bounded* (`open::MAX_BYTES`) and that binary files and non-UTF-8 files are refused with a
sentence rather than mangled. Neither is asynchronous yet, and the doc comments say so.

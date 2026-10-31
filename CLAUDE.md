# CLAUDE.md

Working notes for this codebase. `README.md` is for people using DaCode; this is for whoever is changing
it. `PLAN.md` is the long-form architecture and decision record — when this file and `PLAN.md`
disagree, `PLAN.md` is the reasoning and this is the summary.

## What this is

A code editor in Rust: `winit` + `wgpu` + `glyphon`, no GUI framework. Thirteen crates, ~68,000 lines,
1039 tests, clippy clean.

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

If you add a background producer, give it a `Version`, a `poll()` that never blocks, and a `Waker`
(`dacode::wake`) that it calls from its own thread when it has an answer, and wire its `poll()` into
`App::collect_background` beside the others. The event loop sleeps when nothing moves and nothing is
polled on a timer: a producer without a waker has its result read on the next keypress, which is how
syntax colours once came to appear only after a click. Work with a deadline instead of a thread, like
autosave and the settings write, adds that deadline to the loop's `WaitUntil`. The crates below the
binary cannot name `dacode::wake::Waker`, so `dacode-syntax`, `dacode-project`, `dacode-lsp` and
`dacode-terminal` keep private copies of the alias, which must stay identical to it.

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
dacode-theme    Oklch, twelve themes, type roles (one face: Fira Code), mode accents
dacode-display  soft wrap, ScrollAnchor, Folds, sticky scroll
dacode-buffer   Document = text + undo history + pending changes + pairing
dacode-input    Action, Grammar, Keymap, JumpList, Macros, the command table
dacode-syntax   tree-sitter on a worker thread
dacode-render   Frame -> quads/glyphs/marks, paint Layers, Headless capture
dacode-project  FileIndex, FileWalk, ProjectSearch, FileWatcher
dacode-lsp      transport, client, Diagnostics
dacode-lua      init.lua -> Config
dacode-terminal a shell on a PTY (alacritty_terminal), xterm key bytes, viewport-sized Screen
dacode-import   VSCode data, .vsix, colour schemes in four formats
dacode          the binary: chrome, execute, panes, layout, settings, session, welcome,
             palette, help, menu, clipboard, process, scm (git), source_control (its view)
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
per `n` for stepping.

The parse tree handed to the UI thread (`EditorState::structure`) is the fifth, and is neither moved
nor cleared by `mutate`: it carries the version it was parsed from, and every reader goes through
`fresh_structure`, which refuses it once an edit has moved the buffer on. Moving a tree by hand would
be re-implementing tree-sitter's incremental edit on the UI thread. It *is* cleared on a buffer
change, because a version is a number in one document's sequence and another document can reach the
same number.

The blame (`EditorState::blame`, `crate::blame`) is the sixth, and is the lazy move the search
matches are waiting for. It is per line of the *file*, so moving it per keystroke would be a
document-sized walk per Enter; instead `mutate` appends each change's line arithmetic to an edit log
— constant work, never looking at the blame — and a line is looked up by walking that log back to
where it was when the blame was taken. Only drawing looks up, and only the rows on screen, so the
cost is viewport × the log's cap (256 steps; past it every line reads as unknown and a fresh blame
is asked for). A request in flight keeps a log of its own, so its answer lands with the edits made
while git ran already applied. `latency.rs` holds both halves: a document walk injected into the
recording fails it at 10.6x, and one injected into the column's drawing at 112x.

The expansion history (`EditorState::expansions`, `chrome::Expansions`) is the seventh, and is
**cleared** at the top of `mutate` — before its early return — and on a buffer change: it records
what each selection was before each `expand_selection`, so `shrink_selection` can retrace it, and a
record of how a selection grew means nothing once the text under it has moved. It is also trusted
only while the selections are exactly what the last expansion left, the way `vertical_goal` is. If
you add an eighth, transform it in `mutate` beside the spans, clear it, version-gate it or log it,
and say which.

**A diff tab is a buffer `mutate` refuses.** Clicking a change opens git's unified diff as an
ordinary buffer with no path, marked by `EditorState::review` — which travels with its `Buffer`
through every swap, as the highlights do. Every edit funnels through `mutate`, so that is where read
only is enforced: the closure is run against a throwaway copy and its answer returned, so whatever
asked gets a value of the type it expects and the text on screen stays git's. Gating each action
instead (`edits_text`) would be a list that is wrong the first time an action is added. Saving it
under a name (Save As, `:w path`) clears `review`, because a patch on disk is a file.

**A viewed file is a buffer `mutate` and `write_active` both refuse.** An image, a PDF or an office
document opens as an ordinary buffer marked by `EditorState::preview` (`crate::viewer`), which
travels with its `Buffer` as `review` does. The buffer of a picture is *empty*, so the save refusal
is not politeness: without it `ctrl-s` would replace a PDF with nothing. `open::read` answers
`Opened::View` before its binary sniff, and `App::ensure_preview` — in `after_buffer_change` —
starts the viewer, so a buffer restored as an empty placeholder reads its file only when shown.
Decoding is a worker's; a PDF keeps a worker for its tab's life and renders a page when asked, at
the width it is drawn, and only `Preview::placed` pages — the viewport's — are ever asked for.
Answers are keyed by (page, width): a raster for a zoom since left is dropped. SVG opens as text,
because people edit it; `preview_file` shows the picture. Pictures are drawn by
`dacode-render`'s `image.rs`, which paints over its layer's rectangles, so anything drawn on top of
one goes a layer up.

**DaCode's downloaded tools are on the end of `PATH`, put there once in `main` before any thread
exists.** That one `set_var` is the only place the editor changes its own environment, and it is
safe only because nothing has been spawned yet — move it later and it races every `getenv`. Being
on `PATH` is the whole integration: the server table, the formatters, and the installer's check
afterwards all find a tool by asking for its program, so none of them knows `tools.rs` exists.
Appended, never prepended, so a server the user installed wins. A release download is checked
against a SHA-256 compiled into `tools::TOOLS`; never fetch the sum beside the file, which would
make the check worthless against a replaced asset.

**Every git command the source-control view runs goes through `scm::Worker`, one at a time and in
order** — "stage, then tell me the status" must not answer with the status from before the stage.
Statuses carry a `Version` from `source_control::View::ask` and only the newest asked is accepted; a
status with a newer one queued behind it is answered `Superseded`, so `busy()` returns to zero.
**Nothing may wait for a person:** `GIT_TERMINAL_PROMPT=0`, no stdin, `GIT_EDITOR=true`, ssh in
batch mode unless the user configured it — a push that needs a password fails at once with
`Failure::sign_in`, and the sentence sends them to the terminal. The status itself runs with
`GIT_OPTIONAL_LOCKS=0`, or its refresh of the index would trip the repository watch, which would ask
for another status.

**The repository is watched, and `HEAD` moving re-asks the blame and the gutter.** The view's
status names `.git` (and the common dir in a linked worktree); `FileWatcher::watch_repository`
watches it non-recursively and `refs/` recursively — never `objects/`, which a fetch writes
thousands of. An index write refreshes the list; anything else (a commit, a checkout, a fetch) is
`RepositoryChange::history`, which also raises `blame_wanted` and forgets `diffed_version` —
before, a commit made in a terminal reached the blame only when the file was next saved. Status
requests from the watch `settle` on the worker for 200 ms first, because a checkout is a burst of
writes. `source_control_tests::a_commit_made_outside_the_editor_asks_for_the_blame_and_the_gutter_again`
fails if the watch's history arm is removed.

**The source-control list is built per status, never per frame.** `View::rows` and the group
counts are computed when a status is accepted; `View::geometry` lays out only the rows on screen,
the same as the search view. A status is capped at `scm::MAX_ENTRIES` (5,000) with the total
counted, and "stage all"/"discard all" use pathspecs (`:/`) rather than the list, so they still act
on what the view could not show.

**Everything is an `Action`.** Keys resolve to one, the palette lists them, macros record them, config
binds them, tests drive them. That is why macros survive rebinding, and why `3dw` can be tested
headlessly. Adding a user-visible operation means adding an `Action`, a name in `Action::from_name`,
that name in `Action::NAMES`, and a row in `dacode_input::commands::COMMANDS` — the name is public API
that config depends on, and three tests fail until all four exist.

**What the keys are *now* is `dacode_input::shortcuts`, read through `Command::keys_for` and
`shown`.** The Keys page edits a per-command override layer (`Shortcuts`): a command given keys there
loses its built-in *shortcuts* (ctrl/alt/function keys) and keeps Vim's grammar keys, in both
grammars. The grammar asks it before `dacode.map` and before the built-ins, and drops a built-in
shortcut whose command was moved (`Grammar::unless_moved`, only for a command *begun* with a
shortcut, so `u` still undoes); `terminal::route` asks it too, and the App's pre-grammar intercepts
step aside for a key it claims. The live set is a **thread-local** (`shortcuts::install`/`current`)
rather than a parameter: the palette, help, menus, which-key, tooltips and the page all print keys
through `Command::shown`, and threading a reference through every builder — a dozen in `menu.rs`
alone — was a hundred signatures that could each be handed the wrong one. Per-thread, so tests,
which cargo runs one to a thread, cannot see each other's. `App::new` installs from its `Settings`
(tests pass the default), `App::change_shortcuts` is the one way the App changes it, and the set is
written to `init.lua` as `dacode.keys { … }` by `settings_ui::write_keys` on the settings pause.
While a row waits for a key, `App::route_key` hands it every key before the grammar.

**`commands::COMMANDS` is the only description of what the keys are built as.** The command palette, the help
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
the chrome's bars and buttons, `Menu::geometry`, `welcome::geometry`, `settings_ui::Page::geometry_at`,
`tree::panel`, `layout::CodeArea`, `palette::Palette::geometry`, `help::Help::geometry`. The drawing code reads the same function. A rectangle computed
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

**A panel that scrolls must hand its rectangles to the mouse.** `settings_ui::Page::geometry_at`
returns the visible rows and their boxes at the scroll spring's offset; drawing and hit-testing both
read it (`App::settings_page_geometry`). Teaching the drawing half to
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

**The interface zoom is applied in `App::scale` and nowhere else.** `chrome::Zoom` multiplies the
desktop's scale factor, and every layout and hit test already works in physical pixels through that
one function — so code, chrome, mouse and the terminal grid move together. A site that reads
`window.scale_factor()` itself draws at 100% while the mouse measures at the zoom; the one in the
frame path that cannot call `scale()` (the renderer is borrowed) spells it out and says so.

**One face, one weight — and a screenshot cannot see a substitution.** cosmic-text keeps a requested
family only from a face of *exactly* the requested weight. Fira Code is bundled at 400 alone, so a 500
or 600 request quietly drew the labels, the settings headings and the wordmark in Noto Sans on any
machine with fonts installed — while every capture drew Fira Code, because `DACODE_NO_SYSTEM_FONTS`
left nothing to fall back to. Every role is 400 now and `text::attrs` snaps any weight to a bundled
one; hierarchy comes from size, case, tracking and colour. `DACODE_FONT_TRACE=1` *without*
`DACODE_NO_SYSTEM_FONTS` prints every glyph drawn from another face — run it after touching type. The
guards are the decoy-font tests in `text.rs` and `crates/dacode/tests/typeface.rs`, which checks every
string the chrome draws against the bundled face's character map. A user's own file may still fall
back for CJK or emoji, and should.

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

Flags: `--light --dark --theme NAME --mode M --split N[h] --tree --settings --settings-page NAME
--settings-search Q --theme-dropdown --welcome STEP --folded --minimap
--git --palette Q --search Q --grep Q --complete PREFIX --signature LABEL --diagnostic L:C:MSG --diagnostics-list
--splash --empty --at LINE --at-column N --at-ms N --scroll-px N --row N --columns N --no-wrap --size WxH --no-vim
--transparency PERCENT --zoom PERCENT --help-panel --update-offer KIND --menu --menu-title TITLE --context-menu --commands
--which-key PREFIX --symbols QUERY --block LINE:COLUMN --capture-key NAME[=KEYS] --terminal --terminal-tabs N
--terminal-height PERCENT --terminal-select R:C-R:C`.

`--row N` selects a row of the settings tab, which is the only way to see a section below the fold —
the page scrolls to its selection. `--settings-page NAME` opens it at a page of the tree, and
`--settings-search Q` types a search into it.

`--at-ms N` draws the moment N ms into whatever is animating, and `--scroll-px N` scrolls on by N
pixels, part-way into a row. Both exist because the things they photograph are not states but
instants: a card half-arrived, a top row half-scrolled out. A capture without `--at-ms` is drawn ten
seconds in, past where any spring settles, so it is always the finished screen. `--menu-title Help`
opens the menu bar at a title, or `...` at the overflow's list, which is the only way to look at it
without a window narrow enough to need it.

`DACODE_NO_SYSTEM_FONTS` hides font fallback by construction, so for anything about type, also
capture once *without* it and compare.

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
  conventions.rs` enforces both. The terminal's shell is the one exception: `alacritty_terminal`
  spawns it attached to a ConPTY pseudoconsole, which *is* its console, so there is no window to
  suppress.
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
| The numbers against VSCode | `bench/compare/compare.py` measures both from outside and writes `web/src/benchmark.json`; the feature checklist is `web/src/versus.ts`, by hand |
| How something looks | `crates/dacode/src/chrome.rs` |
| Where something is | `crates/dacode/src/layout.rs` |
| A config option | `dacode-lua/src/lib.rs`, `dacode/src/config.rs`, `dacode/src/settings_ui.rs` (all three) |
| What the title, activity and tab bars offer | `crates/dacode/src/layout.rs` — `title_bar`, `ACTIVITIES`, `TAB_BAR_COMMANDS`, read by drawing and by the mouse |
| The menu bar's titles and what hangs under them | `layout::title_bar` for where the titles are, `crates/dacode/src/menu.rs` for what is in them; `Menu::geometry` reads the first rather than measuring titles itself |
| Something that moves | `crates/dacode/src/motion.rs` for the springs; `EditorState::sync_motion` in `chrome.rs` is where every target is set, and drawing only reads |
| Scrolling | `crates/dacode/src/scroll.rs` — `ScrollInput` and `by_pixels`; the remainder lives on `state.chrome`, and `layout::CodeGrid::with_sub_row` is the only place it enters pixel arithmetic |
| Which surface has the keys | `crates/dacode/src/focus.rs` — `Focus` and `focus_for`; a click sets it, and the key router asks it rather than which panel is open |
| Waking the event loop | `crates/dacode/src/wake.rs` — `Waker`, `Wake` and `coalescing`; `App::collect_background` is the one idle step, and `services_busy` is what still needs a timer |
| The settings tab | `crates/dacode/src/settings_ui.rs` — `Page` is the model (categories, search, scroll, selection, a row's key `Capture`), `Page::geometry_at` its rectangles; `App::save_settings_if_due` and `settings_ui::write_touched`/`write_keys` are the auto-save |
| A command's keys as the user changed them | `crates/dacode-input/src/shortcuts.rs` — `Shortcuts`, `install`/`current`, `TERMINAL_KEYS`; `dacode.keys` in `dacode-lua` |
| Which face a run is drawn in | `crates/dacode-render/src/text.rs` — `attrs`, `shaped_weight`, `fira_code_draws` |
| An icon | `crates/dacode-render/src/icon.rs` — geometry on a 24-unit grid, drawn by `shape.wgsl` |
| The logo | `crates/dacode-render/assets/mark*.png` — coverage masks; `examples/icons.rs` builds every icon from them |
| What a right-click offers | `crates/dacode/src/menu.rs` — `CONTEXT`, names from the command table |
| A theme | `crates/dacode-theme/src/theme.rs` |
| LSP behaviour | `crates/dacode-lsp/src/client.rs`, applied in `crates/dacode/src/edits.rs` |
| Which language server runs | `crates/dacode-lsp/src/lib.rs` — `LANGUAGES`, plus `dacode.server` in config |
| Where the tree's rows are | `crates/dacode/src/tree.rs` — `panel`, same rule |
| Whether a file may be opened | `crates/dacode/src/open.rs` — every refusal is a sentence for the screen |
| Whether a path is inside the project | `crates/dacode-project/src/lib.rs` — `within`, lexical and testable |
| Which buffer is which | `crates/dacode/src/main.rs` — `buffer_key` for identity, `stored_index` for order |
| What survives an edit | `crates/dacode/src/chrome.rs` — `EditorState::mutate` |
| Indentation on Enter, and `af`/`it`/`aa`/`ic` | `crates/dacode-syntax/src/structure.rs`, its queries in `crates/dacode-syntax/queries/`; the lexical fallback is `dacode-buffer`'s `pairs.rs` |
| How a file is written | `crates/dacode/src/save.rs` — `write`, and `App::write_active` for buffers |
| The terminal | `crates/dacode-terminal` for the shell, keys and `Screen`; `crates/dacode/src/terminal.rs` for which keys it keeps and its colours; `layout::terminal_panel` for where it is; `App::handle_terminal_key` and `poll_terminal` in `main.rs` |
| The source-control view | `crates/dacode/src/source_control.rs` — `View` (rows, message box, selection, questions) and `View::geometry`/`hit`, read by `chrome::draw_source_control` and `App::click_source_control`; keys in `App::handle_scm_key` |
| What git is asked, and how | `crates/dacode/src/scm.rs` — `parse_status` (porcelain v2), `parse_branches`, `Op`, `Worker`; `App::sync_scm` collects, `App::git_action` dispatches every `Action::Git` |
| A diff tab | `App::show_review` opens it, `EditorState::review` marks it, `EditorState::mutate` keeps it read-only, `chrome::diff_line_kind` colours it |
| Updating DaCode | `crates/dacode/src/update.rs` — the release JSON, `install_kind`, `SHA256SUMS`, staging and `apply`, behind a `Fetch` trait so tests never reach the network; `App::poll_updates`, `update_action` and `restart_for_update` in `main.rs`; `chrome::notifications_panel` for the offer's buttons; `.github/workflows/release.yml` writes the sums |
| A viewed image, PDF or document | `crates/dacode/src/viewer.rs` — `Kind`, `Preview` and its worker, `office_text`; `chrome::draw_preview`, `App::ensure_preview`, `handle_preview_key`; pictures in `crates/dacode-render/src/image.rs` |
| A downloaded formatter or language server | `crates/dacode/src/tools.rs` — `TOOLS` (pinned versions and SHA-256s), `install`, `run_cli` for `dacode tools`; `dacode_lsp::SELF` is the installer that runs it, `App::offer_download` the Languages page's click |
| Who changed a line | `crates/dacode/src/blame.rs` — the porcelain reader, the edit log, `Tracker` and `BlameWorker`; `App::sync_blame` asks and collects, `chrome::line_note` decides between a diagnostic and the inline blame |

Adding a config option touches three places by design: the Lua reader, the settings struct, and the
settings page. If you add it to fewer, it will silently not work somewhere.

## Known gaps

Deliberate, with reasons in `PLAN.md`: column advance derived from the type scale rather than
measured from the shaper, no macOS packaging. Cross-file rename is not
undoable and says so.

From the syntax-tree work: indentation and the structural text objects read the tree only while its
version is the buffer's, so Enter pressed within a parse of the last keystroke — fast typing — gets
the lexical rules, and `daf` in that moment does nothing and says the tree is catching up. The indent
query also hands back to the lexical rules inside a parse error, which is usually the line being
typed. Only the structural objects take a count — `2af` is the function around this one, and `2aa`
the argument of the next list out, never "two arguments" — while `2iw` and `2i(` still ignore theirs.
Shrink-selection retraces the expansions it can see and otherwise steps to the named node under
the caret, so shrinking a selection made by dragging picks a child rather than the one you meant.

Smaller ones, from the non-modal work: a drag after a double-click extends by character rather than by
word; and the which-key popup sits at the top left of the code area rather than following the cursor,
because a popup that moved as you typed would be harder to read than a still one. The palette's and
the help panel's rows are clickable now, through `Palette::geometry` and `Help::geometry`, which the
drawing reads too; a click runs the row through the same function Enter does, so it leaves focus
where Enter would.

Every overlay is drawn on `chrome::card`. `chrome::elide`, `elide_start` and `wrap_to` measure with
`layout::measured_advance` — the face's real 0.615 em rather than the type scale's 0.60, which let a
line measured to fit exactly run past its box and be cut mid-word — and they are used everywhere, so
tabs, the tree, the statusline and the settings rows elide a little sooner than they did. So do the
key chips, the menu's columns and the palette's and completion's match highlights. Not yet: the
title bar's own measurements, which pad the type scale by hand (1.1x, 1.15x), and the code grid,
which stays at 0.60 on purpose — moving it moves every glyph of code and every click into it.

From the terminal: tabs, mouse selection (alacritty's own `Selection`, so it follows scrollback and
is dropped exactly when alacritty drops it), a draggable height and mouse reporting all exist; what
does not is splitting a terminal, reordering tabs, reporting pointer *movement* with no button held
(`?1003` gets presses, drags and releases only), and OSC 52 clipboard writes, which go unanswered on
purpose. OSC 10/11/4 colour queries are answered from the theme. Every key goes to the shell while
it has focus except `` ctrl-` ``, `ctrl-t`, `ctrl-shift-c`, `ctrl-shift-v`, `ctrl-shift-p`,
`ctrl-pageup`, `ctrl-pagedown` and the zoom keys `ctrl-=`, `ctrl--`, `ctrl-0` — see
`dacode::terminal::route` before adding another. `ctrl-t`
costs the shell readline's transpose-characters and `ctrl-pageup`/`pagedown` cost Vim its tab keys;
`ctrl-w` deliberately stays the shell's word delete. The panel's height is a fraction on
`terminal::Panel`, persisted in the session rather than as a setting (it is where you left things,
per project, and a setting would rewrite `init.lua` on every drag); `layout::terminal_height` is the
one clamp, read by the layout and the drag alike. While the settings page is in front and the
terminal does not have the keys, `EditorState::terminal_folded` leaves the panel only its header,
and the shells are not resized for it; giving the terminal the keys unfolds it. Closing a tab hangs its shell up and alacritty then waits for it without limit; bash can swallow
that SIGHUP while blocked reading the terminal, so the reaper kills a shell still there after
`HANG_UP_GRACE` (two seconds) — that swallowed hang-up was the terminal tests' "flake", which failed
6 runs in 10 under parallel load and 0 in 20 after. Each tab's shell has its own dirty flag and the
shared waker, so a busy hidden tab wakes the loop once and never again until it is shown.

From the motion and scrolling work, each one stated rather than hidden:

- **Overlay clicks use the settled rectangles.** A card scales in from 0.96 over about 300 ms, and
  hit-testing follows logical state rather than the scaled picture, so for that moment a click near
  the edge of a card is measured against where the card will be. The alternative, hit-testing the
  animated rectangle, would make a click mid-flight miss a row the pointer is plainly on.
- **Switching `animations` to Reduced does not re-base springs already in flight.** `set_animations`
  takes no instant to evaluate them at, so a spring that was running when the setting changed
  finishes as it was, overshoot included. Switching to Off snaps everything, so this affects only the
  one direction.
- **The wheel glide is not re-based after an edge clamp.** A glide that runs into the top or bottom of
  the file keeps aiming at the target it was given, which the view cannot reach, until it settles.
- **Only the focused pane has a sub-row offset.** `draw_panes` gives the others `0.0`, so they sit on
  whole rows, and `restore_pane` settles the remainder as focus moves. A split therefore never shows
  two panes part-way into a row, and a touchpad stream in one pane never shifts another.
- **Pinch-to-zoom works only where winit delivers `PinchGesture`**, which is macOS. On Wayland and
  Windows, ctrl-scroll is the way, and the keymaps guide says so.
- **The Keys page moves shortcuts, not Vim's keys, and does not show `dacode.map`.** An override
  replaces a command's ctrl/alt/function-key bindings and leaves `u`, `dd` and the rest alone; a
  sequence must start with `ctrl-k`; and per-mode `dacode.map` bindings run but are printed nowhere.
  The App's own intercepts for `ctrl-p` and `ctrl-b` are not commands, so giving one of those keys
  away is not warned about. A `dacode.keys` written outside the page's block is copied into it, and
  resetting it on the page leaves the hand-written line to apply again at the next start.
- **Restoring `init.lua`'s recovery copy after a quit brings back the old settings block.** A setting
  held for an `init.lua` with unsaved edits is written to the file on disk at quit, while those edits go
  to the recovery file, so restoring that copy puts the block from before the change back over it.
- **LSP and format edits do not set `last_edit`**, so they never trigger autosave. This predates the
  overhaul; the autosave deadline is simply measured from the last keystroke.
- **With the results panel open and focus in the editor, Esc does not close the results.** Esc goes
  to the surface that has the keys, and the panel is not it; spec section 5 chose this over the old
  behaviour, in which an open panel took every key. Click the panel, which gives it focus, and Esc
  closes it and hands the keys back.

Electric dedent gives up after `DEDENT_SCAN_LIMIT` characters, so a `}` whose `{` is more than four
or five hundred lines above it is left where it was typed. It used to walk to the start of the
document, which cost 5.8 ms per keystroke in a hundred-thousand-line file. Doing nothing is what
already happened when no opener was found, so there was nothing new to invent.

Sticky scroll looks at most `dacode_display::HEAD_SCAN_LIMIT` (500) lines up for a block head, so a
block longer than that pins nothing. It used to walk to line 0, every frame: 57 ms a frame halfway
down a hundred-thousand-line file. Every frame guard in `latency.rs` drew from the top of the file,
where the walk is free; the minimap's draws from the middle, which is how it was found.

From the source-control view: diffs are unified, not side-by-side, and nothing is staged by hunk or
line. One repository per folder — the one the project folder is in — so submodules and several
repositories side by side are not separate entries. Stash has no "include untracked" and no choice
of which stash to pop. A pull or push in flight cannot be cancelled; it runs off the UI thread, so
nothing waits on it, but it has to finish or fail. The message box keeps its caret but has no
selection or undo of its own. The Run title is gone from the menu bar — Git took its place, since a
ninth title pushed Help off at 1600px — and the run command is under Terminal and on the tab bar's ▶.

From the updater: **there is no code signing.** A download is checked against `SHA256SUMS` from the
same GitHub release over HTTPS, which catches corruption and a swapped asset but not a compromised
release — whoever can replace the tarball can replace the sums. Releases made before the updater
carry no `SHA256SUMS` and are refused for self-install, so the first updater release must be
installed by hand. HTTP goes through `curl` rather than a TLS crate, so a machine without it is told
so instead of updated. "Restart now" runs the release's own `install.sh` on the UI thread as the
editor leaves — a few file copies, after the session and recovery are written; on Windows the setup
runs with `/SILENT` and its "Start dacode" entry is `skipifsilent`, so DaCode is not reopened there.
A `/usr/local` or `/opt` install, or one in a prefix outside the home directory, is told the release
exists and nothing more. The automatic check is skipped for a cargo build (it would never offer
anything), and `resumed` is the only caller, so no test or capture reaches the network.

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

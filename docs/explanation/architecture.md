# How DaCode is put together

This is an explanation, not a manual. Nothing here tells you which key to press or which option to
set — [the reference](../reference/) does that. This is for the reader who wants to know why the
editor behaves the way it does, and for anyone thinking about changing it.

Almost every structural decision in DaCode descends from a single constraint, so that is where to start.

## The one rule

**Nothing unbounded may ever run on the UI thread.**

That is the whole design brief. An editor feels slow not because it is slow on average but because
it is occasionally slow — a keystroke that costs ten microseconds nine hundred times and two hundred
milliseconds once is remembered as the two hundred. The way that happens is almost always the same:
some piece of work is proportional to the size of the document rather than to the size of the
change, and nobody notices until the document is large.

So the rule is absolute rather than a budget. Work whose cost grows with the file does not belong on
the thread that draws. Three consequences follow, and between them they account for most of the
architecture.

### Background work is version-tagged, and stale results are thrown away

Syntax highlighting, language-server traffic, git diffs and the project file walk all run on other
threads. Each is handed a cheap snapshot of the rope — cloning it is O(log n) and shares structure, so a
snapshot is not a copy of the text — together with the document's `Version` at that moment.
When the worker finishes, it returns its result **tagged with the version it saw**.

The UI then does the only safe thing: it compares that tag against the document's current version,
and if they differ it discards the result entirely. Not partially, not with a best-effort fixup — a
set of highlight ranges computed against text you have since typed into describes a file that no
longer exists, and merging it would put colour on the wrong words. Throwing it away costs one more
round trip; merging it costs correctness.

This is also why the workers never block. A background producer offers a `poll()` that returns
whatever is ready and nothing else, and the event loop calls it on every turn. The editor asks; it
never waits.

### Anything per-frame is bounded by the viewport, not the document

The second consequence is quieter and easier to break. Drawing a frame must only ever touch what is
on screen. Roughly forty rows are visible at a normal window size, and a frame that walks a hundred
thousand of them to compute something about forty is a frame that gets slower as your file grows.

The one real performance defect this project has found was exactly that shape: a sidebar walked 512
display rows to compute a single coordinate. Bounding it to the visible rows took frame build from
**1.13 ms to 84 µs**. Nothing had failed — the benchmarks had been measuring that code for months
and said nothing, because nothing was comparing two document sizes against each other.

What the rule buys, measured on ordinary hardware:

| | 1k lines | 100k lines |
|---|---|---|
| Keystroke (grammar plus execute) | 6.6 µs | 7.5 µs |
| Frame build (layout plus text runs) | 84 µs | 85 µs |
| Viewport walk (40 rows) | 71 µs | 70 µs |

The interesting property of that table is not that the numbers are small. It is that each column is
the same as the one beside it.

### The rule is a test that fails the build

`crates/dacode/tests/latency.rs` is the rule written down as something that can go red, and its design
is worth explaining because the obvious version does not work.

The obvious version asserts milliseconds. That fails, and it fails in the worst way: a loaded CI
runner is several times slower at everything, so any threshold tight enough to catch a real
regression is also tight enough to go off at random on a busy afternoon, and a test that cries wolf
gets ignored or deleted. An absolute budget is a property of the machine, not of the code.

So the test asserts **flatness** instead. It measures the same operation on a thousand-line document
and on a hundred-thousand-line one — a hundredfold difference — and requires that the larger costs
at most three times more. Three rather than one, because a bigger rope genuinely is a little slower:
deeper tree, worse cache locality. The claim being made is "not proportional to the document", not
"identical". A ratio cancels the machine out, so the same assertion is meaningful on a laptop and on
a shared runner.

Absolute ceilings are there too, but set where only a catastrophe trips them — 500 µs for a
keystroke, 8 ms for a frame build. Something that turned a keystroke into whole-document work would
blow past those on any hardware; nothing else will.

Every measurement is a median of thousands of runs rather than a mean, because a median ignores the
scheduler stealing one iteration.

And the guard was verified the way every guard in this project has to be: by injecting the violation
it exists to catch. A document-length walk added to `chrome::draw` fails it at **104x**. One added to
`InsertText` fails it at **111x**. Restoring the old electric-dedent scan, which used to walk back to
the start of the file looking for a matching brace, fails it at **96x**. A guard nobody has watched
fail is a guard nobody knows works.

## The twelve crates

DaCode is a Cargo workspace of twelve crates, arranged so that dependencies only ever point one way —
down this list. Nothing below reaches back up.

| Crate | What it owns |
|---|---|
| `dacode-text` | The rope (`ropey`), the typed coordinate types, `Edit`/`Transaction`, `Anchor`, `SelectionSet`. |
| `dacode-theme` | Oklch colour, the thirteen built-in themes, the type roles, the per-mode accents. |
| `dacode-display` | Soft wrap, `ScrollAnchor`, folds, sticky scroll — the mapping from buffer lines to display rows. |
| `dacode-buffer` | `Document`: text plus undo history plus pending changes plus bracket pairing. |
| `dacode-input` | `Action`, the modal `Grammar`, `Keymap`, the jump list, macros, and the command table. |
| `dacode-syntax` | Tree-sitter on a worker thread, version-tagged. |
| `dacode-render` | `Frame` to quads, glyphs and marks; the paint layers; the headless capture path. |
| `dacode-project` | `FileIndex`, `FileWalk`, `ProjectSearch`, `FileWatcher` — gitignore-aware discovery and search. |
| `dacode-lsp` | The JSON-RPC transport, the client, diagnostics, and the language table. |
| `dacode-lua` | Reads `init.lua` and hands back a plain `Config` value. |
| `dacode-import` | VSCode data, `.vsix` packages, colour schemes in four formats. |
| `dacode` | The binary: chrome, execute, panes, layout, settings, session, welcome, palette, help, menu, clipboard, process. |

The ordering is not bureaucracy. `dacode-text` knows nothing about display, which is why coordinates in
the rope cannot accidentally acquire a notion of what is on screen; `dacode-lsp` sits below the binary,
which is why hover and rename can be tested with no server running at all.

## The ideas that shape the code

### Positions have types

`ByteOffset`, `CharOffset`, `Utf16Column`, `Point` and `DisplayPoint` are five distinct types, and
none of them converts to another implicitly.

This looks like ceremony until you notice how many things in an editor count differently. LSP speaks
UTF-16 code units, because that is what JavaScript strings are made of. The rope speaks characters.
The screen speaks display rows, which are not buffer lines once soft wrap or folding is involved,
and columns that depend on the tab width. A file containing an emoji has a position that is one
character, two UTF-16 units and four bytes, all at once, and every one of those numbers is correct
for a different question.

Bare integers are how editors get their worst bugs, because the wrong one is usually right. It is
right for ASCII, right for most of a test suite, right until somebody puts an accented letter in a
comment and the language server starts underlining the character next door. Making them separate
types moves that whole class of defect from runtime to compile time.

### There is exactly one way to change text

Every edit goes through `Document::apply(Transaction, EditKind)`. Nothing else touches the rope.

`apply` returns the exact inverse of what it did, which is what undo is made of — undo is not a
re-simulation or a snapshot diff, it is the inverse transaction that was computed at the moment the
edit happened. `EditKind::Discrete` marks an edit as its own undo group, which is what makes a
rename, a format, a macro replay or a snippet expansion undo as the one thing the user thought they
were doing, however many individual edits it took.

Inside the binary there is a layer above that: `EditorState::mutate`. The document is the funnel for
text, but not everything that describes the file lives inside it. Folds live in the display map and
are line numbers, and line numbers do not move themselves. Deleting a function above a closed fold
left that fold's range running off the end of the file, and since the fold only asks whether a line
falls inside its range, everything below the fold's head vanished. `mutate` takes a closure, runs it
against the document, and then replays the resulting changes over the folds. There is a test that
greps the workspace for text changed outside it, because "remember to call the other thing too" is
not a rule that survives a year — and that grep found two places hand-editing had already missed.

The general form: **nothing outside the document may hold a bare line number across an edit.** If
you add something that does, it gets transformed in `mutate` beside the folds, or cleared, and the
code says which.

### Everything is an `Action`

There is one vocabulary of things the editor can do. Keys resolve to an `Action`; the palette lists
`Action`s; macros record them; config binds them; tests drive them.

Two useful properties fall out of that, neither of which was designed in separately. Macros survive
rebinding, because a macro is a list of actions rather than a list of keystrokes — remap a key and
your recorded macros still do what they did. And a modal sequence like `3dw` can be tested
headlessly, because feeding keys through the real grammar produces actions and executing actions
needs no window.

The cost is that adding a user-visible operation means adding it in four places: the `Action`, its
name in `Action::from_name`, that name in `Action::NAMES`, and a row in
`dacode_input::commands::COMMANDS`. Three tests fail until all four exist. That is deliberate — the
name is public API that people's config files depend on, and the command table is what the palette,
the help panel, the menu bar and the which-key popup all read. Four features maintaining four
separate lists would disagree within a month, and a discoverability feature that lies is worse than
none: it teaches a key that does nothing.

### Layout is data

`crates/dacode/src/layout.rs` is the single definition of where things are on screen. Drawing reads it;
mouse hit-testing reads the same function.

The failure this prevents is specific and recurrent. If the code that draws a row and the code that
decides what a click hit each compute the rectangle themselves, the two copies drift, and the
symptom — clicks landing a line off from where they look — gets blamed on the display map or the
scroll position, which is where nobody will find it. It has been broken three times here and each
break presented as a different bug: a function that quietly shrank its own rectangle for sticky
scroll, so clicks landed up to three lines low but only in files where something was pinned; and a
file tree whose click handler left no bottom padding where the drawing did, so clicking one file
opened another.

The rule that came out of it is stronger than "share the arithmetic": **a function that returns rows
must return their rectangles too**, and any splitting of a rectangle happens before either the
drawing or the hit-testing half runs. The same discipline governs `settings_ui::panel`,
`tree::panel`, `menu::geometry` and `welcome::geometry`, and it is why the horizontal scroll offset
enters pixel arithmetic through exactly one type, `layout::CodeGrid`.

### Paint layers

The renderer draws all quads and then all glyphs, **per layer**. There are three: `Base` for the
editor itself, `Panel` for things that sit over the code — hover, completion, search results — and
`Modal` for overlays that cover everything, like the command palette.

Within a single layer, a quad cannot occlude a glyph, because all the glyphs are drawn last. That is
what makes horizontally scrolled code stop at the gutter rather than painting over the line numbers,
and it is also how the completion popup once shipped transparent: its background was on the same
layer as the text underneath it, so the text won.

## How it is verified

The whole appearance renders offscreen. `dacode --screenshot out.png` builds one frame, writes a PNG
and exits, with flags to put the editor into any state worth looking at — a theme, a mode, a split,
the file tree, the settings tab at a chosen page or row, the first-run wizard at a chosen step, folded
blocks, the git gutter, any of the palettes, the completion popup, a diagnostic, the menu bar, a
right-click, a half-typed chord. [The CLI reference](../reference/cli.md) lists them all.

CI renders every one of these on each push and keeps the PNGs as an artifact. That matters more than
it sounds: it means the visual check runs on a machine with no GPU and no compositor, and it means a
change to the chrome produces a picture somebody can look at rather than a diff somebody has to
imagine.

This is how most of the visual defects in this project were actually found. A completion popup that
was transparent. Tofu boxes where the bundled font had no glyph for a character. A logo that came
out as an X. Diagnostic messages clipped mid-word. A lock icon two pixels out of alignment. And, on
the very first CI run that produced pictures, the F1 help panel drawing its entire key list into a
thirty-pixel strip at the foot of its card — because `split_bottom` returns the bottom piece first
and `split_top` returns the top, which is invisible in the code and unmistakable in the image.

One habit goes with it: **render at more than one size.** The harness was fixed at 1600x1000 for a
long time, and a whole family of defects lives below that — the settings rows collided with the
footer under about 900 pixels of height, and the toolbar was laid out at a negative x under about
450 pixels of width. Neither was reproducible at the default. Four overflow defects shipped for
exactly this reason.

## What it deliberately does not do

**No JavaScript runtime, and therefore no extension code.** This is the largest single omission and
the most considered one. VSCode extensions are JavaScript written against the `vscode` API, running
in Node; supporting them means embedding a JS runtime, reimplementing that API, and paying a
Node-sized process per extension — which is precisely what makes VSCode heavy, and the opposite of
this editor's premise. There is a legal blocker too: Microsoft's marketplace terms restrict it to
their own products, which is what other editors hit in 2025, so the registry DaCode targets is Open VSX.

What is portable is extension **data**, and `dacode import` takes it: themes, snippets, keybindings,
language configuration. What most valuable extensions really wrap is a language server, and DaCode
drives those over LSP directly, so the loss is smaller than the omission sounds.

**No integrated terminal, no git blame, no structural text objects** (`if`, `ic`). Each is deferred
with reasoning rather than forgotten; `PLAN.md` has the account.

And the limitations that are stated rather than hidden, because an honest limitation is better than
a clever workaround:

- **Buffer search is still linear in the document.** It no longer copies the rope — it scans a line
  at a time, so memory is bounded — but a rare query in a very large file still walks all of it on
  the UI thread. The fix has the same shape as syntax and git: a worker with a `Version` tag.
- **Opening a file is still synchronous.** What changed is that it is now *bounded* — 128 MiB, via
  `open::MAX_BYTES` — and that binary files and non-UTF-8 files are refused with a sentence rather
  than mangled.
- **Only the active buffer gets a file watcher, a git diff and a language server.** Background
  buffers get crash-recovery writes and nothing else, so a file changed on disk behind an unfocused
  buffer is noticed when you switch to it rather than when it happens. The alternative is N watchers
  and N servers; this is a deliberate trade, but it is a limitation.
- **Nobody has ever run DaCode on macOS.** It has no macOS packaging, and while the path resolution has
  a non-Windows branch that macOS would take, that branch has never been exercised there.

## Further reading

[`PLAN.md`](../../PLAN.md) is the long-form record: the milestones, the measurements, the
architecture arguments, and the decisions that turned out wrong, kept because the record is the
point. [`CONTRIBUTING.md`](../../CONTRIBUTING.md) is how to build it, test it, and stay inside the
conventions; [`CLAUDE.md`](../../CLAUDE.md) is the terse working summary of the same, invariant by
invariant.

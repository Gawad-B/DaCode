# vex — Project Plan

**Target:** VSCode's feature surface, Zed's performance characteristics, Neovim's modal keybindings.
**Language:** Rust. **Primary success criterion:** I use it as my daily editor.

## Locked decisions

| Decision | Choice | Consequence |
|---|---|---|
| Name | **vex** — binary `vex`, config `~/.config/vex/init.lua` | crate prefix `vex-` |
| Soft wrap | **In v1**, designed into the display layer at M2 | `DisplayPoint` is wrap-aware from the start; viewport math is row-based, not line-based |
| Selection model | **Always a set** (Zed/Helix data model); a cursor is a 1-wide selection with one primary | Every operator maps over the set; multi-cursor is never a special case |
| Command grammar | **Vim grammar** (`dw` — operator then motion) **plus Helix-style extras** where strictly better (structural tree-sitter selection, split-selection, multi-cursor ops) | Operators must resolve a motion into a per-cursor selection; two entry paths into one operator implementation |
| Config | **Lua from the start** (`mlua`) | **Done** (`crates/vex-lua`). The `Action` enum is a public scripting API from day one — naming and stability matter earlier than they otherwise would. Two things kept the accepted cost in check: the interpreter is *transient* (it runs once at startup, returns a plain value and is dropped, so no Lua ever runs on the UI thread), and bindings resolve to `Action` names rather than Lua callbacks, so a config cannot inject work into the keystroke path. What that forgoes for now is autocommands and user-defined commands; those need a callback budget and get one when they are built. |
| Languages | Rust, TypeScript/JS/HTML/CSS, Python, C/C++, Dart | tree-sitter grammars + LSP: rust-analyzer, tsserver + vscode-langservers, pyright/ruff, clangd, `dart language-server` |
| Milestone order | **M5 (project layer) before M6 (LSP)** | Navigation friction blocks daily use before missing completion does |
| Toolchain | rustup stable (1.97.1), edition 2024 | — |

---

## 0. Framing and scope reality

Zed is roughly five years of work by a funded team; VSCode is over a decade. That does not
mean this project is unreasonable — it means the goal must be **"the editor I use daily"**,
not "feature parity". A single-user editor can skip an enormous amount: extension APIs,
remote development, settings sync, notebooks, debugging UI, multi-platform polish,
accessibility, telemetry, backward compatibility.

The strategy is therefore:

1. Build a **correct, fast text core** — this is the part that is genuinely hard to retrofit.
2. Get to **self-hosting as fast as possible** (editing this project inside the editor).
   Every day spent self-hosting surfaces real priorities and kills imagined ones.
3. Add the VSCode-shaped features **in the order my own workflow demands**, not in the
   order a feature comparison table implies.

**Explicit non-goals for v1:** a plugin/extension system, remote/SSH editing, integrated
debugger, collaborative editing, Windows/macOS support (Linux first — that is the dev box),
themes marketplace, settings GUI.

Deferring the plugin system is the single most important scope decision. It is what makes
VSCode enormous, it constrains every internal API once it exists, and a solo user does not
need it. Revisit only after the editor is genuinely in daily use.

---

## 1. Architecture

### 1.1 Core principle: the "60fps contract"

The single rule that produces Zed-like feel: **nothing on the UI thread may ever be
unbounded in cost.** Concretely:

- Keystroke → visible glyph must be bounded work: rope edit + re-render of visible lines
  only. No full-file scans, no full-file re-parse, no disk I/O, no allocator storms.
- Everything unbounded (parsing, LSP, search, file watching, git, disk) runs off-thread and
  communicates results asynchronously.
- Background results are computed against an **immutable snapshot** of the document and are
  **version-tagged**; stale results are discarded, never applied.

This rule drives the data structure choices below and is the reason the buffer must be
cheaply cloneable.

### 1.2 Layer diagram

```
┌─────────────────────────────────────────────────────────────────┐
│ Shell / App          window, event loop, focus, layout, panes    │
├─────────────────────────────────────────────────────────────────┤
│ UI                   editor view, gutter, statusline, palette,   │
│                      file tree, search panel, completion popup   │
├─────────────────────────────────────────────────────────────────┤
│ Input                key decoding → modal state machine → Action │
├─────────────────────────────────────────────────────────────────┤
│ Workspace            documents, views, selections, undo history, │
│                      registers, marks, jumplist, config          │
├─────────────────────────────────────────────────────────────────┤
│ Text Core            rope, edit transactions, coordinate spaces  │
├─────────────────────────────────────────────────────────────────┤
│ Services (async)     syntax (tree-sitter), LSP, search, fs watch,│
│                      git, formatters                             │
├─────────────────────────────────────────────────────────────────┤
│ Render               shaping + GPU text rendering                │
└─────────────────────────────────────────────────────────────────┘
```

Dependency rule: layers only depend downward. Text Core knows nothing about UI. Services
never touch UI state; they emit messages the Workspace applies.

### 1.3 Key technical decisions

| Concern | Decision | Why / alternatives |
|---|---|---|
| Text storage | `ropey` | O(log n) edits, cheap clone-on-write snapshots for background work, correct char/byte/line index conversion. Alternative — hand-rolled gap buffer — is a fine learning exercise but a liability for a daily driver. |
| Window + input | `winit` | De facto standard, matches the wgpu ecosystem. |
| GPU | `wgpu` | Portable, mature, what Zed-class renderers are built on. |
| Text shaping/raster | `cosmic-text` + `glyphon` | Real shaping (ligatures, complex scripts, fallback) + a glyph atlas on wgpu. Writing shaping by hand is a multi-month detour with no payoff. |
| Syntax | `tree-sitter` | Incremental, error-tolerant, gives structure (not just colors) for text objects and indent. Regex-based highlighting is a dead end. |
| Async runtime | **None** — `std` threads + channels | Superseded the original `tokio` choice. The background work is a handful of long-lived workers (syntax, LSP, file walk, project search), each a thread that owns a blocking loop and reports over a channel. An async runtime buys nothing for four threads and costs a large dependency tree and a colored-function boundary through the whole editor. |
| Logo rendering | A dedicated distance-field pipeline (`mark.wgsl`) | The mark is the one graphic element that is not a rectangle: diagonal strokes need antialiasing, and its 34% echo would double-blend into a visible seam if drawn as overlapping translucent quads. Evaluating the stroke outline as a signed distance solves both at once — a union is a `min`, so overlap is free, and coverage is computed once. Alternatives rejected: an image pipeline plus a rasterizer for one shape; approximating the diagonals as stair-stepped rectangles; drawing it as two `v` glyphs, which would put the identity at the mercy of a font fallback. |
| Paint order | Three layers: base, panel, modal | The renderer draws all rectangles and then all glyphs, so within one layer a panel cannot cover text drawn before it — every overlay was transparent. Each layer is now drawn completely (its quads, then its glyphs) before the next. Alternative — one draw call per panel — costs a pipeline switch per overlay for no gain at three layers. |
| LSP | `lsp-types` + own client | The protocol plumbing is a weekend; the semantics are the work. |
| Search | `grep-searcher` + `grep-regex` + `ignore` | The ripgrep crates. Do not write project-wide search by hand. |
| Fuzzy matching | `nucleo` | Helix's matcher; fast and good ranking for the command/file palette. |
| Config | Lua (`mlua`) for the user's own config; a hand-rolled TOML subset for what the VSCode importer writes | Superseded the original `toml` + `serde` choice on both sides. Lua because config *is* code — bindings map keys to the `Action` vocabulary, and the useful ones are generated or conditional. The importer's output stays key/value because it is machine-written and machine-read, and a TOML crate plus derive macros for a hundred lines of that is compile time and binary size an editor premised on staying light should not spend. `init.lua` is read last and wins. |
| Error handling | `anyhow` at boundaries, `thiserror` in libraries | Conventional. |

**GUI framework, deliberately rejected:** `egui` / `iced` are immediate-mode or
retained-widget frameworks whose text handling is not built for an editor's needs (per-glyph
styling, huge documents, block cursors, virtualized scrolling). `gpui` (Zed's) is powerful but
is developed for Zed's own use and is rough to build on externally. Owning
`winit + wgpu + glyphon` is more initial work but removes the ceiling.

### 1.4 Crate layout (Cargo workspace)

```
crates/
  text/        rope wrapper, Point/Offset/Anchor, Edit, Transaction   (no deps on UI)
  buffer/      Document: text + undo history + syntax + diagnostics
  input/       key events, modal state machine, keymap resolution
  actions/     the Action enum + dispatch — the single vocabulary of "things the editor does"
  syntax/      tree-sitter integration, highlight queries, indent queries
  lsp/         client, server registry, capability handling
  search/      buffer search + project search
  render/      wgpu pipeline, glyph atlas, quad/text batching
  ui/          views, layout, painting, panels
  editor/      binary: wiring, config, window, event loop
```

`actions/` deserves emphasis: **every** user-visible operation is an `Action` value. Keymaps
resolve to Actions, the command palette lists Actions, macros record Actions, and tests drive
Actions. Getting this right early is what keeps Neovim-style composability from becoming a
tangle of special cases.

### 1.5 Two designs to get right the first time

These are expensive to retrofit; everything else can be refactored.

**(a) Coordinate spaces.** Editors are bug factories because "position" is ambiguous. Define
distinct, non-interchangeable types from day one and never use bare integers:

- `ByteOffset` — for tree-sitter and disk
- `CharOffset` — for rope operations
- `Point { line, column }` — logical position, column in chars
- `DisplayPoint { row, column }` — after soft-wrap, folds, tabs, inlay hints
- `Anchor` — a position that survives edits (biased left/right), for marks, diagnostics,
  selections, and background results

LSP uses UTF-16 code units, tree-sitter uses bytes, ropey prefers chars, rendering needs
display coordinates. Make every conversion an explicit, tested function.

**(b) Edits and undo.** One path for all mutation: `Transaction` = an ordered set of edits +
selection-before + selection-after + a version number. Applying a transaction returns the
inverse. Undo is then a stack of inverses, grouping is a policy over transactions, and
multi-cursor is simply a transaction with many edits. If any code mutates the rope outside
this path, undo and background invalidation will both break.

### 1.6 Modal editing model

Model on Neovim's grammar rather than imitating keys ad hoc:

```
pending: [count] [register] operator [count] {motion | text-object}
```

Implement as an explicit state machine producing a resolved `Action`. Support Normal, Insert,
Visual (char/line/block), Replace, and a command line. Design for these from the start
because each breaks naive implementations:

- **Counts and registers** as first-class prefixes, not flags bolted on.
- **Text objects** (`iw`, `ap`, `i(`) — tree-sitter makes structural ones (`if` function,
  `ic` class) nearly free, which is a genuine improvement over stock Vim.
- **Dot-repeat** — requires that the last change be a replayable Action, which the
  Action-centric design gives you.
- **Multi-cursor** interacting with modal commands — Neovim lacks this and Zed/VSCode have it;
  decide early that a "selection" is always a set, with one primary.

Do **not** embed Neovim or Vimscript. Do not aim for plugin compatibility.

---

## 2. Milestones

Each milestone has a demo and an acceptance bar. Do not advance until the bar is met — this is
what prevents a half-working engine with ten half-working features on top.

> **Status.** M0–M7 done but for an integrated terminal, and M8's config hot-reload, plus the VSCode
> importer, a measured optimization pass, Lua configuration and the logo. 452 tests, zero clippy
> warnings.
> Crates:
> `vex-text` (rope, coordinates, transactions, anchors, selections — 13 tests),
> `vex-theme` (OKLCH color, mode accents, type roles, theme tokens — 15 tests),
> `vex-buffer` (document, versioning, undo grouping, multi-cursor editing — 19 tests),
> `vex-display` (soft wrap, anchored viewport, display-aware motion, folding, sticky scroll — 31 tests),
> `vex-input` (Action vocabulary, modal grammar, motions, text objects, user keymap, jumplist, macros — 82 tests),
> `vex-syntax` (tree-sitter parsing, incremental reparse, version-tagged worker — 18 tests),
> `vex-render` (wgpu quad pipeline + glyphon text, paint layers, frame timer, offscreen capture — 10 tests),
> `vex-import` (VSCode settings/keys/themes/snippets, `.vsix` reading — 19 tests),
> `vex-project` (gitignore-aware discovery, fuzzy ranking, ripgrep search, file watching — 25 tests),
> `vex-lsp` (JSON-RPC framing, process lifecycle, UTF-16 positions, diagnostics, rename, formatting — 17 tests),
> `vex-lua` (`init.lua`: options, theme tokens, key bindings, error reporting — 14 tests),
> `vex` (window, chrome, executor, config, panes, file tree, git gutter, sessions, snippets, edit
> application, `vex import` — 154 tests).
>
> **Everything is an `Action`.** Keys resolve to Actions, the executor consumes them, dot-repeat
> replays them, and the end-to-end tests drive them — so `3dw` and `ci"` are verified as behavior,
> headlessly, with no window or GPU. That is the payoff of §1.4's Action decision, and it arrived
> exactly where predicted.
>
> **The viewport is anchored, not indexed.** Absolute display-row numbers would need either an
> O(document) scan or an incrementally-maintained sum tree, so the viewport is instead a buffer
> line plus a row offset within that line's wrapped rows, and a screenful walks forward from
> there. That keeps scrolling O(visible rows) at any document size, which is what §1.1 requires.
> The cost: no cheap absolute row number, so scrollbars use buffer lines as a proxy.
>
> **The async contract is now real, not aspirational.** The UI thread hands the syntax worker a
> rope snapshot plus its version and never waits; the worker returns results tagged with the
> version it saw; the UI applies a result only if that version still matches. Sustained typing is
> handled by splitting the work — **every** version is parsed (tree-sitter's incremental reparse
> needs each edit in the coordinates it was expressed in, so skipping one leaves the tree
> describing text that never existed), but only the newest queued version is *queried*, since
> intermediate highlight results would be stale on arrival. Parsing is the cheap half.
>
> Only Rust has a grammar so far. Files without one render unhighlighted rather than not at all.
>
> **Still stand-ins, deliberately:** structural text objects (`if`/`ic`) exist in `vex-syntax`
> (`enclosing_node`) but are not yet bound in the grammar; no macros, marks or jumplist; no search
> (`/`) or command line (`:`); tree-sitter indent queries are unused, so `insert_newline` still
> just copies the previous line's whitespace; the column advance is derived from the type scale
> rather than measured from the shaper; and Lua config is decided but unwired.
>
> **Fonts are bundled.** Instrument Serif (Regular + Italic), IBM Plex Mono Regular and IBM Plex Sans
> Medium are embedded in the binary with `include_bytes!` — 480KB, both families SIL OFL with licence
> texts committed beside them. The editor now renders its own design on a machine with nothing
> installed, which is what the handoff asked for.
>
> Loading them costs **0.20ms**. The system font scan that follows costs **~156ms** and buys the
> fallback coverage four Latin faces cannot provide — CJK, emoji, anything else. Correctness wins by
> default; `VEX_NO_SYSTEM_FONTS=1` takes the startup back (0.27s → 0.19s to first frame) for anyone
> who only edits Latin text.
>
> **Verifying the look.** `vex --screenshot out.png [--mode insert|visual|replace] [--light]
> [file]` renders one frame offscreen and exits — no window server needed, so the design can be
> checked in CI, over SSH, or in a headless VM. This is also the basis for reference-image tests
> once the chrome settles.
>
> Two bugs the first screenshot caught immediately, both invisible to the unit tests:
> letter-spacing was being scaled by pixel size when cosmic-text expects **em** (which collapsed
> the wordmark's letters into one blob and blew the tracked labels apart), and panel prose had no
> wrapping. This is the argument for offscreen capture existing at all.

### M0 — Spike: prove the render path (1 week)
Window via winit, wgpu surface, draw a monospace grid of glyphs from a hardcoded string at a
stable frame rate; resize correctly; handle HiDPI.
**Bar:** measured frame time under 5ms for a full screen of text.
**Purpose:** de-risks the least familiar layer before any architecture is committed.

### M1 — Text core (1–2 weeks)
`text` and `buffer` crates: rope, coordinate types with conversions, `Transaction`, undo/redo
with grouping, anchors that survive edits.
**Bar:** property tests — random edit sequences where (i) undo-all returns the original text,
(ii) anchors track the correct character, (iii) all coordinate round-trips agree. This is the
foundation; test it like a library, not like a feature.

### M2 — Minimal usable editor (2–3 weeks)
Open/edit/save a file. Cursor, scrolling, viewport culling (render only visible lines),
selection, clipboard, line numbers, statusline.
**Bar:** open a 100MB file and a 1M-line file; scroll and edit with no perceptible lag.
Large-file behavior is a design property, not an optimization to add later.

### M3 — Modal editing (2–3 weeks)
Normal/Insert/Visual, motions, operators, counts, registers, text objects, dot-repeat,
`:` command line, search with `/` and `n`/`N`.
**Bar:** **switch to it for real editing.** From here on, the editor is dogfooded daily and
its own bug tracker.

### M4 — Syntax (1–2 weeks)
tree-sitter with incremental reparse on a background thread against version-tagged snapshots;
highlight queries; theme file; tree-sitter-driven indent.
**Bar:** typing in a 10k-line file never blocks; highlighting is never applied stale.

> **M5 so far.** File discovery honors `.gitignore` (ripgrep's `ignore`), fuzzy ranking uses
> `nucleo-matcher`, and project search uses ripgrep's own engine — all on background threads, all
> bounded. The command palette is implemented per the design and **owns the keyboard while open**
> (typing `i` searches for `i`; it does not enter insert mode). `ctrl-p` opens files, `ctrl-b`
> switches buffers, and each buffer keeps its own undo history and scroll position.
>
> Two bugs the smoke test caught, both of which unit tests could not: `find_root` returned
> `crates/vex` because the first `Cargo.toml` walking up is a workspace *member* manifest — `.git`
> now wins, so the palette indexes the project rather than one crate; and the file walk was
> collected *after* key handling, so the first `ctrl-p` would have opened onto an empty list.
>
> **M5: splits and panes.** A flat list divided along one axis, not a tree — nesting is a cost every
> layout and focus question pays, for an arrangement few people build deliberately. `ctrl-w v` / `s`
> split, `ctrl-w c` closes, `ctrl-w w` cycles. Each pane keeps its own buffer, scroll position and
> selections, so moving between them is returning rather than reopening.
>
> Two decisions worth recording. **Highlights travel with the buffer**, not with the live document:
> an unfocused pane cannot be edited, since keys only reach the focused one, so the spans computed
> when it was last focused are still correct for its text. That is what lets a split show two
> *highlighted* files with a single syntax worker. And **only the focused pane draws a cursor, a
> cursor line and a mode-tinted gutter** — a second blinking cursor is the most confusing thing a
> split view can do, and an unfocused pane is not in insert mode just because its neighbour is.
>
> The index arithmetic lives in `crates/vex/src/panes.rs` rather than the event loop, because
> arithmetic only reachable by pressing keys in a window is arithmetic that never gets tested.
>
> **M5 is complete.** The file tree is derived from the project's flat index rather than a second
> walk, so it can never disagree with the file palette about what is in the project. File watching
> watches the *directory*, not the file: editors and formatters save by writing a temporary file and
> renaming it over the original, which replaces the inode and leaves a file watch following a ghost.
> A clean buffer reloads silently; a dirty one is never touched, and says so instead.
>
> Replace is scoped to the results panel rather than the whole project — you replace in what you can
> see, having read it — and rebuilds its matches by re-reading each file rather than trusting the byte
> offsets the search recorded, which describe the file as it was.
>
> One protocol bug found on the way: reloading a changed file builds a new `Document`, whose version
> restarts at 1. Versions are how every asynchronous consumer tells fresh from stale, and LSP requires
> them to increase — so a reload was telling the server to go backwards, after which every diagnostic
> it returned read as stale forever. `Document::resume_from` keeps the numbering monotonic.

### M5 — Project layer (2 weeks)
Multiple buffers, splits/panes, file tree, fuzzy file palette, command palette, project-wide
search and replace, file watching.
**Bar:** navigate this project entirely by keyboard without a shell.

> **M6 so far.** A working client: JSON-RPC framing, process lifecycle (`initialize` → `initialized`
> → `shutdown` → `exit`), full-document sync, and diagnostics shown in the statusline with the
> version-tagged staleness rule applied. Verified against **real rust-analyzer**, both in a test that
> completes a handshake in ~4s and in the running editor.
>
> Two deviations from §1.3, both deliberate:
>
> - **No `tokio`.** A language server is a child process and two pipes; threads and channels model
>   that directly, in the same shape the syntax worker already uses. An async runtime would be a
>   large dependency serving no need here.
> - **Full document sync, not incremental.** Incremental sync sends less but requires client and
>   server to agree on every position, and one disagreement corrupts the server's copy *silently* —
>   producing diagnostics for text that does not exist. Correctness first; optimize on a measurement.
>
> A finding worth keeping: `~/.cargo/bin/rust-analyzer` is a **rustup shim** that exists whether or
> not the component is installed. `is_installed` returning true is not proof a server works, which is
> why the client keeps the server's stderr and surfaces it — that is how the first integration failure
> explained itself instead of looking like a vex bug.
>
> **M6 is now user-facing.** `K` hovers, `gd` goes to the definition (opening another file when the
> target is there), and `]d` / `[d` walk the diagnostics with the same wrapping `n` uses. Diagnostics
> are underlined beneath the offending span — a rule rather than a wash, because a wash over an error
> makes the thing you are being told about *harder* to read — and the message for the diagnostic under
> the cursor appears in a box that flips above the cursor when there is no room below, so it never
> hides what it describes.
>
> The split worth noting: `execute` records a *request* on the state and the event loop sends it,
> because the executor must not own a process handle. That is what lets hover and goto-definition be
> tested with no language server running at all.
>
> `K` displaced the old add-caret-above binding, which moved to `gA`: `K` for hover is what every
> Neovim LSP configuration binds, and matching that is the point of "Neovim's keybindings".
>
> **M6 is complete but for one thing.** Code actions arrive in the *command palette* rather than a
> menu of their own — it is a list you filter and choose from, which is what the palette already is —
> and only actions carrying edits are listed: an action whose work is a server-side `command` is
> something vex cannot perform, and offering a menu entry that does nothing is worse than offering
> nothing. References land in the *search results panel* for the same reason: both are `path:line`
> plus a line of context that you move through and open, and two implementations of that would drift.
>
> Inlay hints are off by default and requested once per document *version* rather than on a timer, so
> typing does not queue a request per keystroke and a reply is never for text that has moved. They are
> drawn in the gaps rather than shifting the code sideways — a hint is information *about* the code,
> and code that moves as hints arrive is code you cannot read.
>
> The diagnostics list answers a question the underlines cannot: `]d` walks them one at a time, which
> is useless for deciding whether a file is in trouble at all.
>
> **M6 is complete.** Servers are now kept per program rather than one at a time, so a project mixing
> Rust and TypeScript keeps both alive; switching to another file's language and back does not restart
> anything, which would otherwise throw away minutes of rust-analyzer's indexing every time you glance
> at a file. Only the active buffer's server is talked to — the others hold their state and their
> index, costing memory but no work. Verified for the one-server case (switching between Rust files
> and files with no server does not restart it); a second server was not installed to test the pair
> live, which is stated rather than implied.

### M6 — LSP (3–4 weeks)
Client + lifecycle, diagnostics, hover, goto-definition/references, completion,
rename, code actions, format-on-save.
**Bar:** rust-analyzer working end-to-end on this repo. Note: completion and diagnostics are
where the async architecture is truly tested — expect this milestone to expose latent design
mistakes, and budget for that rather than being surprised by it.

> **M6: rename and formatting.** Both go through one place — `crates/vex/src/edits.rs` — because the
> hard parts are shared and worth getting right once. A rename touching nine sites is **one** undo
> press, which falls out of the document's single mutation funnel: every edit enters as one
> `Transaction` tagged `Discrete`. A reply computed against a version the buffer has moved past is
> **refused whole** rather than merged, and the open buffer is checked *before* any other file is
> written, so a stale reply cannot half-rename a source tree. Edits that overlap are rejected as a set
> — applying the valid half would leave a file mangled in a way no undo entry describes. Files that
> are not open are written atomically and are **not** undoable, which is a real limitation, so the
> editor reports how many files it touched rather than implying otherwise.
>
> Two findings from testing against a live rust-analyzer, which is the only honest test of this:
> a rename reply that would not parse was being dropped with `.ok()?`, making a failed rename
> indistinguishable from a dead key; and the first request reliably came back `ContentModified`
> because the server was still indexing. The protocol marks that error retryable, so it is now its own
> event with a `retry` flag and the editor says "the server is busy — try again" instead of nothing.
> It does not retry by itself: a request that re-fires unasked is how an editor hammers a busy server.
>
> Format-on-save writes first and reformats second. The alternative — hold the save until the
> formatter answers — either blocks the UI thread or risks a save that never lands, and a save must
> land on disk when asked. Off by default, because a formatter reflowing a file you did not ask it to
> is worse than no formatter.
>
> **M7 so far.**
>
> - **`/` search and `:` commands.** A Vim-style prompt line that takes over the statusline, with
>   incremental highlighting as you type, a live match count, `n`/`N`, and cancel restoring the
>   original cursor position — a search that loses your place is worse than no search. Commands:
>   `:w`, `:w path`, `:q`, `:q!`, `:wq`/`:x`, `:42`, `:e path`, `:grep`, `:set wrap`/`nowrap`. An
>   unknown command names itself, so a typo is distinguishable from an unimplemented feature.
> - **Crash recovery.** Dirty buffers are written to `$XDG_STATE_HOME/vex/recovery` on a two-second
>   timer and forced before quitting, so a crash costs seconds rather than work. Writes are atomic
>   (temp file plus rename) because a crash *during* the recovery write would otherwise replace a
>   good copy with a truncated one. A copy older than the saved file is discarded rather than
>   offered. Scratch buffers are not covered — they have no identity to recover to.
> - **Project-search UI.** A streaming results panel below the editor: `path:line` plus the matching
>   line, `ctrl-f` for the word under the cursor or `:grep` for anything else, `↑↓` to move and `↵`
>   to open at the line. Capped at 500 hits, with the count marked `500+` so it never reads as
>   complete.
> - **Seven more grammars.** Python, JavaScript, TypeScript, TSX, C, C++, HTML and CSS alongside
>   Rust. Two findings, both caught by a test that loads every grammar and highlights a real sample:
>   TypeScript's query is *not self-contained* (by convention it is combined with JavaScript's, and
>   alone it highlights only types), and CSS/HTML use a markup capture vocabulary — `tag`,
>   `property`, `@media` — that fell through to `Text` and produced **no highlights at all** until
>   mapped. Dart is deferred: no maintained crate is published, and its language server works over
>   LSP regardless.
>
> Also fixed here: the statusline reported `rust` for every file, whatever its language.
>
> - **Auto-pairing and indentation.** Typing `(` supplies `)`, typing the closer steps over it rather
>   than doubling, backspace between an empty pair removes both halves, a pair around a selection
>   wraps it, Enter between `{` and `}` opens an indented body with the closer below, and a closer
>   alone on a line dedents to match its opener. Per-language: Python indents after `:`, Rust does not
>   pair `'` (it is a lifetime far more often than a character literal).
>
>   **Deliberately lexical, not syntax-aware.** Indentation must be decided on the keystroke, and the
>   syntax tree lags by design — worse, at the instant Enter is pressed the line being opened does not
>   exist in any tree. So the rules read the previous line's indentation and the delimiters around the
>   cursor: bounded, instant, and predictable in a way a version-behind tree is not. Tree-sitter indent
>   queries remain the better answer for pathological cases and would need a synchronous parse of the
>   current line — a separate decision.
>
>   The rules are conservative because auto-pairing that fires unwanted costs more keystrokes than it
>   saves: `(` before an identifier does not pair, and a quote never pairs mid-word.

> **M7: folding, git gutter, jumps, macros, sessions.** Two of these were only possible because every
> operation is an `Action`: a macro is a list of Actions and a jump is one that declared itself
> far-reaching, so neither needed a new mechanism. That is the payoff §1.4 predicted for defining the
> vocabulary before the code that produces it, and it arrived exactly where predicted.
>
> Macros record **actions, not keystrokes**. Keystrokes would replay the grammar as well as the
> command, so a macro recorded under one keymap would do something else under another — and a macro
> that breaks when you rebind a key is a macro you cannot trust. Replay is one undo step, because a
> macro is one intent however many edits it makes.
>
> The git gutter shells out to `git show HEAD:path` rather than linking a git library: the editor
> needs exactly one thing from git, and linking `gix` would add an object-database implementation to
> an editor premised on staying light while still having to agree with the `git` binary about
> worktrees and submodules. The diff is ours, version-tagged like every other background result.
>
> Snippet expansion closes a loop left open since M5: the importer could already *read* snippets and
> said so in its report. `tab` expands, then walks the tab stops, then falls back to indenting — in
> that order, so it keeps being the indent key for everyone with no snippets. The stops are anchors,
> so they move with the text typed into them. Deliberately not a live template with mirrored
> placeholders: those need an edit path running on every keystroke, and every editor that has one has
> a history of it firing inside a string literal you were editing.
>
> Sessions are separate from crash recovery on purpose: recovery must survive a `kill -9` and so is
> written every two seconds, while a session is only interesting when the editor closed properly and
> is written once, on the way out.

### M7 — Daily-driver gaps (ongoing, 3–4 weeks)
Whatever is actually blocking use: soft wrap, folding, multi-cursor, git gutter and blame,
integrated terminal, session restore, jumplist, macros, autopairs, snippets, sticky scroll,
inlay hints.
**Bar:** two weeks with no reaching for another editor.

> **Everything in M7 is done except the integrated terminal and git blame**, and the terminal is
> deferred deliberately rather than half-built. A terminal is a PTY, a VT-sequence parser and a
> second rendering path with its own scrollback, selection and colour model — a subsystem the size of
> several crates here, whose failure mode when done badly is a shell that eats your keystrokes. The
> editor already sits in a terminal for anyone who wants one, and `ctrl-z` is not a worse answer than
> a bad terminal emulator. It gets built when it is the thing actually blocking use, which is the bar
> this milestone sets. Blame is a smaller gap: the gutter machinery is there and `git blame --porcelain`
> would feed it, but per-line authorship earns its screen space far less often than per-line change.

> **The folder chooser shells out.** vex draws its own interface with `wgpu` and has no GTK or Qt in
> it; linking one — or `rfd`, which links one for you — to show a single dialog would add a second UI
> stack, theme engine and event loop to an editor premised on staying light. So it runs `zenity`,
> `kdialog` or the XDG portal, whichever exists, **on a thread**: a dialog is a subprocess someone may
> leave open for a minute, and waiting on the UI thread would freeze the editor behind it. `zenity` and
> `kdialog` are preferred over the portal deliberately — both are one command that prints a path,
> a contract easy to be sure of, while driving the portal without a D-Bus library means racing a signal
> against a method call and parsing it out of a text stream. Typing a path remains the fallback when
> nothing is installed, because a key that silently does nothing is worse than one that asks.

> **M8: a settings panel.** Config is a Lua file, and a panel that *replaced* it would have to write
> the user's program for them. So the panel does something a file cannot: its third column says which
> of default, `init.lua` or imported-from-VSCode last set each value. When a setting is not what you
> expect, that is the only useful question, and showing you one of three files does not answer it.
>
> Changes apply immediately — a settings panel whose effect you cannot see is a form — and are written
> back only when asked, into a marked block at the end of `init.lua`. The user's own lines are never
> rewritten: a `vex.set` call may be inside a loop or a condition, and an editor that reformats your
> program to store a checkbox has overstepped. Appending works because `init.lua` applies in order, so
> the panel's block is simply the last word. A test round-trips what the panel writes through the real
> Lua reader, because a block that will not parse loses every setting on the next launch.
>
> It covers the options safe to change blind — sizes, widths, switches, appearance. Themes and
> keybindings stay in the file, where a colour picker and a key capturer would otherwise have to be.

> **M8: config hot-reload.** `init.lua` is watched like any other file and re-read on save. Theme,
> metrics and bindings are replaced; nothing about the *documents* is touched — a reload that reset
> scroll positions or dropped undo history would make editing your own config hostile, which is
> exactly when the feature is used.

### M8 — Polish and performance (ongoing)
Startup-time budget, frame-time instrumentation, memory profiling, crash-safe recovery of
unsaved buffers, config hot-reload, documentation.

**Realistic total to a genuine daily driver: 4–6 months of steady part-time work.** M0–M3 is
the hard, unglamorous part and roughly half the difficulty.

---

## 3. Risks and how the plan absorbs them

| Risk | Mitigation |
|---|---|
| Rendering/shaping is unfamiliar and could sink the project | M0 spike isolates it in week one, before commitment |
| Async correctness (stale highlights, races) | Snapshot + version tag convention decided in §1.1, enforced from M4 |
| Unicode/grapheme/emoji bugs | Typed coordinates + `unicode-segmentation`; test with CJK, emoji, ZWJ, combining marks, RTL from M2 |
| Retrofitting undo or coordinates | §1.5 — these two are designed before features exist |
| Scope creep toward VSCode parity | Non-goals in §0; M7 is driven strictly by observed friction |
| Motivation loss in the long middle | M3 puts the editor into daily use early, which converts the project from abstract to self-reinforcing |
| Large files / minified JS pathologies | Explicit acceptance bars in M2; long-line handling tested deliberately |

---

## 4. Engineering practices

- **Property tests** for the text core (`proptest`) — the highest-value tests in the project.
- **Golden/scripted tests** for editing: a keystroke sequence + before/after buffer state.
  Because everything is an `Action`, these run headless with no GPU.
- **Benchmarks** (`criterion`) for rope edits, highlight reparse, search; a frame-time
  overlay toggled by a key.
- **Fuzzing** the modal state machine with random key sequences — it must never panic.
- **No `unwrap` in the event loop.** A daily driver must not lose unsaved work; auto-persist
  dirty buffers to a recovery directory.
- Set up git and CI (fmt, clippy, test) on day one.

---

## 5. First concrete steps

1. `git init`; create the Cargo workspace with the crate skeleton from §1.4.
2. Write `crates/text` types (`ByteOffset`, `CharOffset`, `Point`, `DisplayPoint`, `Anchor`)
   with conversions and property tests — before any rendering.
3. In parallel, run the M0 render spike as a scratch binary.
4. Decide the config file schema early (keymap, theme, LSP servers) so features have a home.

---

## 5a. VSCode compatibility — what is possible, and what is not

**VSCode extensions cannot run in vex.** Two independent blockers, neither of which is an
engineering matter:

1. **Legal.** Microsoft's marketplace terms restrict use to "in-scope products" — Visual Studio,
   VS Code, Codespaces, Azure DevOps. Non-Microsoft editors may not fetch from it; this is what
   Cursor and Windsurf hit in 2025. **Open VSX** (Eclipse Foundation) is the legally
   redistributable, API-compatible registry, and is what vex targets.
2. **Architectural.** Extensions are JavaScript against the `vscode` API in Node. Supporting them
   means embedding a JS runtime, reimplementing that API, and paying a Node-sized process per
   extension — precisely what makes VSCode heavy, and the opposite of this project's premise.

What *is* portable: extension **data** (themes, snippets, language configuration) and the
**language server** most valuable extensions merely wrap, which vex will drive over LSP directly.

`vex import` implements that. Nothing is imported unless explicitly listed — the checklist defaults
to taking nothing, because an importer that quietly rewrites configuration is worse than none. Every
item is accounted for in a report: imported, skipped, not present, or unsupported *with the reason*.
An imported theme is converted into OKLCH, so it joins the same palette relationships as a native
one and gets a legibility check (illegible imports are corrected and the correction is reported).

Deferred within the importer: snippet *expansion* (parsing works; the tab-stop engine does not
exist), language configuration (needs an auto-close/comment-toggle engine, M7), and language servers
(needs the LSP client, M6). All three are reported as unsupported rather than silently accepted.

## 6. Visual design

Source: `Vex modal state design/design_handoff_vex/` — implement **`vex Editor Paper.dc.html`**
only. The landing page (`vex Landing.dc.html`) is out of scope for now, and
`vex Editor.dc.html` ("Ink") is an unchosen alternate.

**The mock is a reference, not a target to mirror.** It is HTML standing in for a native
`winit`/`wgpu`/`glyphon` UI, so what carries over is the visual *language* — palette,
relationships, type roles, behavior — while geometry is guidance the render layer is free to
resolve its own way. Nothing here gets ported as markup or wrapped in a webview.

Encoded in `crates/vex-theme`:

- **Mode is the environment.** One `Mode` value drives every accent through `Theme::accent`.
  Four hues (normal 250, insert 155, visual 70, replace 25) at **one lightness and one chroma**
  — that shared L/C is what makes the modes read as one instrument changing state instead of
  four themes, so it is asserted in a test rather than left to convention. Nothing downstream
  may hardcode a mode hue.
- **Color is stored in OKLCH**, not hex, because the identity is a relationship between hues;
  conversion to linear RGB happens where the GPU needs it.
- **Shape carries the mode before color does** — block+underline (normal, never blinking),
  blinking bar (insert), range fill (visual), solid block (replace). This survives
  colorblindness and low-light screens.
- **Type contrast is the point**: serif prose and *italic serif comments set larger than the
  code*, against monospaced code, with small tracked uppercase sans labels. Gutter figures are
  serif and must share the code's row height exactly.
- **Square corners, 1px rules**, one shadow (the palette), no images or icon fonts — every
  graphic element is a rectangle.
- **The mark is identity, not state.** The logo — two mitred chevrons, the second a 34% echo one
  step down — appears in exactly three places (title bar at 26px, the cold-start lockup at 46px, the
  empty state at 64px) and nowhere else: never in the statusline, the gutter, the palette, or over
  text. It is also the one element in the chrome immune to the mode hue, which is asserted by
  rendering all four modes and comparing the pixels. Source and rules: `logo/`.
- **Overlays occlude.** A hover, completion popup or palette is opaque over the code, which is a
  renderer property rather than a color one: see the paint-layer row in §1.3. Verified by rendering
  each overlay offscreen and looking at it, which is how the transparency was found in the first
  place.
- **Instrumentation is live, not decorative**: frame time against a 16.6ms budget, and an async
  ledger showing each background producer's document version as fresh or stale. That ledger is
  §1.1's contract made visible, and is how the contract gets debugged.

## 6a. Performance, measured

Benchmarks live in `crates/vex-syntax/benches/parse.rs` and `crates/vex/benches/editor.rs`. They
exist because the 60fps contract is a claim about numbers, and three of my own guesses about where
time went were wrong.

**The contract holds where it matters.** UI-thread cost per keystroke is *flat with document size*:

| Measurement | 1k lines | 100k lines |
|---|---|---|
| keystroke (grammar + execute) | 6.6 µs | 7.5 µs |
| frame build (layout + text runs) | 84 µs | 85 µs |
| viewport walk (40 rows) | 71 µs | 70 µs |

**The one real defect found: the sidebar walked 512 display rows every frame** to compute a single
coordinate, while the viewport showed ~40. Bounding it to the viewport took frame build from
**1.13 ms to 84 µs — a 92.5% reduction.**

Two guesses that were wrong, kept here because the record is the point: streaming the parser from
rope chunks instead of allocating a full document copy gained only ~5% (the copy was ~1% of parse
time), and the rope-backed query provider actually *regressed* highlighting by 3–5%. Both were kept
anyway — they remove an allocation that scales with file size — but neither was the bottleneck.

**Off-thread cost is allowed to exceed a frame.** An incremental reparse of a 20k-line file is
~4.7 ms (vs ~73 ms cold, so incrementality works; the residual scales with top-level item count).
That runs on the syntax worker and is version-tagged, so the UI never waits for it. Tree edits
themselves are ~1.9 µs.

**What the mark costs.** One extra pipeline and one instanced draw call per layer that contains a
mark, which in practice is one. Frame build is unchanged at **84µs** and the icon adds 6.3KB to the
binary (the 256px PNG, decoded once at startup with the `png` crate that was already there for
offscreen capture, rather than pulling in `image` for one file).

**What Lua costs.** The locked decision to configure in Lua was worth checking rather than assuming,
since "lightweight" is a requirement: the vendored Lua 5.4 static library is **738KB** before linking
(the linker keeps far less), and running the shipped example `init.lua` — six bindings, a loop, a
theme table — makes **no measurable difference to startup**: 0.17s either way over three runs each.
That is the payoff of the interpreter being *transient*. It runs once, hands back a plain value and
is dropped before the first frame, so there is no Lua on the UI thread to budget for at all.

A caution learned here: `criterion`'s `iter_batched` includes *dropping* the input in its
measurement. Dropping a `Highlighter` frees a syntax tree and a compiled query, which made a 1.9 µs
operation appear to cost 5.6 ms. Benchmarks that construct expensive state per iteration must use a
long-lived object and a reversible operation instead.

## 7. Open questions to settle before M2

- **Soft wrap in v1?** It complicates `DisplayPoint` and viewport math significantly. If yes,
  design it into the display layer at M2 rather than adding it in M7.
- **Multi-cursor semantics** under modal operators — decide the model before implementing
  operators in M3, since it affects every one of them.
- **Config format:** TOML (simple, static) vs. an embedded scripting language (Lua, powerful,
  and a soft path back to a plugin system). TOML is the recommendation for v1.

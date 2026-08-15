# Changelog

Every release of vex, newest first, written for someone deciding whether to upgrade: what changed
and — where it is not obvious — what the old behaviour cost.

The format is [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html). The long-form account of a
release, where one exists, lives in `packaging/notes/`.

## [Unreleased]

## [0.4.2] - 2026-08-15

Everything here came from one person using 0.4.1 on their own code for an evening, which is where
0.4.1 came from too. Three of them are the same defect wearing different clothes: something the
editor holds *outside* the document, describing text by where it sits, and nothing moving it when
the text moves.

### Fixed

- Typing no longer smears the colours of the rest of the file. Highlight spans are char offsets into
  the text, held outside the document, and nothing moved them when you typed — so between a keystroke
  and the parse that answers it, every span after the cursor described text that had already moved.
  Typing a hundred characters into a line painted the file below it in the colours of the text a
  hundred characters earlier: comment italics running through code, the wrong font on half a line. It
  corrected itself the moment you stopped typing, which is what made it look like a rendering fault.
- A diagnostic's underline stays under the code it is about. The same defect and the longest-lived,
  because a diagnostic is a round trip to another process rather than a parse: the squiggles from the
  previous answer sat over whatever had slid into their place for as long as the server took. Moving
  them is done in their own line-and-column coordinates rather than by converting each one through
  the rope, which is the difference between 39.6 ms and about a tenth of a millisecond for one
  keystroke in a large file full of them.
- A file created outside the editor appears without the folder being opened again. The index was
  built by exactly one walk, at startup, and nothing ever asked for a second — so a file arriving
  from another program, a `git checkout`, or a browser download was invisible to the file tree and
  the palette until the folder was reopened. The directories the project's files live in are watched
  now, and anything appearing or disappearing in one starts another walk. Taken from the walk's own
  result, so `.gitignore` decides what is watched: a recursive watch on the root would put one on
  every directory under `target/` too.
- JavaScript is served as JavaScript. It shared the TypeScript entry, and the entry is what decides
  the `languageId` sent in `didOpen` — so every `.js` file was announced to the server as
  TypeScript, and a server told that applies TypeScript's rules to it. From the outside that is
  indistinguishable from the server not supporting the file. They are separate entries running the
  same program now, so it costs no second install, and the settings panel has a JavaScript row where
  before it had none to find.
- The landing page's release grid fills its last row. Its rules between cards are 1px gaps over a
  background, so a row that stopped short showed that background as a slab of rule colour. It was
  exactly nine cards when it was written, which is what `grid-9` says, and the 0.4.2 card was the
  tenth. The release count above the grid is counted from the table now, so it cannot go on claiming
  a number that is no longer true.

### Changed

- `vex.server` accepts the name a language's own users call it: `"js"` reaches the JavaScript entry,
  `"c"` reaches clangd. Both previously failed with "unknown language", which reads as
  "unsupported".

### Added

- `packaging/linux/uninstall.sh`, and the same script inside the tarball. The per-user installer
  writes a GTK icon cache, GTK trusts that cache over the filesystem, and nothing removed it — so
  deleting a `~/.local` install by hand left an index still promising a `vex` icon at a path that no
  longer existed, which then shadowed the icon a later `.rpm` or `.deb` put in `/usr`, because
  `~/.local` is searched first. It showed as a blank tile with the name under it, in the launcher and
  beside the running window, with a correct icon sitting one directory further down the search path.
  Recovering from it by hand is `gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor`.

## [0.4.1] - 2026-08-15

Everything here came from one person installing 0.4.0 and using it for an afternoon, which found
more than two rounds of adversarial review had. Long-form notes: [`packaging/notes/0.4.1.md`](packaging/notes/0.4.1.md).

### Added

- A language server can be installed from inside the editor, by running the command vex was already
  telling you to type: rust-analyzer through rustup, the TypeScript and pyright servers through npm,
  basedpyright and pylsp through uv, pipx or pip — whichever your machine actually has. It never
  runs a shell, never asks for root, and refuses once before doing anything, so the first request
  prints the exact command and the second runs it.
- The installer's exit code is not treated as proof: vex looks for the program afterwards and says
  so if it is still not there, because on Windows both pip and `go install` write to directories
  `PATH` does not cover. clangd, lua-language-server and the Dart SDK keep advice rather than a
  button, since no single command is honest for them.

### Fixed

- With Vim keys off, Escape no longer puts you in a mode there is no way out of. There is one mode,
  but Escape resolved to "collapse selections", which set normal mode unconditionally — a blue block
  cursor and a blue gutter while the statusline went on saying EDITING, and nothing in the non-modal
  keymap could type the `i` that would have got you out.
- The Linux icon is visible on a light application menu. The desktop entry and all nine icon files
  were correct; the art was a near-white chevron on transparency, which on a pale panel is not a
  faint icon but a blank tile labelled "vex". The rpm and deb now also refresh the icon cache, and
  an entry missing from that cache falls back to the same blank tile.
- The toolbar no longer runs SETTINGS into COMMANDS on Windows. Button widths came from a constant
  of 7.5 pixels per character, below the editor's own estimate for the type it draws them in and
  tied to nothing, so it could not follow the font size either; Linux resolves narrower fonts and
  hid it.
- The mouse wheel scrolls whatever is under it. The settings panel, the key list, the command
  palette, the file tree, the search results and the problems list all ignored it while the file
  behind them scrolled instead.
- A message wraps instead of stopping mid-word, and goes away when you do something else rather than
  waiting for Escape. The box split only on hard newlines, so a one-paragraph message was one line
  however long, clipped where the rectangle ended with no ellipsis — you were told the beginning of
  why something failed and left believing that was all of it.

## [0.4.0] - 2026-08-14

Two halves: the features 0.3.0 left half-built, and an adversarial review of everything behind them.
Long-form notes: [`packaging/notes/0.4.0.md`](packaging/notes/0.4.0.md).

### Added

- Find and replace in the open buffer.
- Alt-drag for rectangular selection.
- Horizontal scrolling, with one definition of where a column is in pixels that the drawing and the
  mouse both read.
- Tabs you can close, and file create, rename and delete from the tree.
- Crash recovery for *every* dirty buffer rather than only the one on screen, and a problems panel
  that keeps every file's diagnostics rather than the active file's.
- Go to a symbol in the file or across the project.
- Formatting with no language server at all, through a worker that runs ruff, black, prettier and
  the rest.
- Language servers can be named from `init.lua`, and a configured entry goes ahead of the built-ins
  rather than replacing them.
- Git: walk the gutter hunk by hunk and revert one change at a time.

### Changed

- Completion uses what the server actually sent, including auto-import. That turned out to be a
  *handshake* gap rather than a parsing one — rust-analyzer withholds out-of-scope candidates
  entirely unless the client advertises `resolveSupport`, so the list came back looking perfectly
  normal with the one useful entry missing.

### Fixed

Thirty-three defects confirmed by two adversarial review passes. Three of them could lose work:

- Typing in the search box could kill the editor. Case-folding `İxÅ` keeps the same total byte
  length while moving every offset inside it, so searching that line sliced mid-character and
  panicked on the UI thread — taking every unsaved buffer with it.
- A crash lost the unsaved work in every file but one. Recovery copies were written for all dirty
  buffers, but session restore never looked one up, and the first save then deleted it.
- `d5G` deleted to the end of the file instead of to line five.
- Folds never moved when the text under them did, so a delete above a closed fold could hide the
  rest of the file.
- Clicks landed up to three lines low wherever sticky scroll had pinned a heading, which read as
  intermittent because nothing is pinned at the top of a file.
- Typing `}` walked to the start of the document to find the opener, at 5.8 ms per keystroke in a
  large file. The scan is now bounded.
- One file opened two ways became two buffers with separate undo histories, and two files deep in
  different trees could collide.

### Security

- `x/../../../../evil.txt` passed the "is this inside the project?" check and `create_dir_all`
  walked it happily. Containment was decided by the filesystem, which cannot answer for a file that
  does not exist yet — the normal case for one being created. It is decided lexically now, folding
  `.` and `..` and refusing any climb above the root.

## [0.3.0] - 2026-08-14

Two releases' worth of the same lesson: the editor kept describing an editor other than the one
underneath it. The rpm's `%changelog` lists a 0.2.1 above this; it was never tagged, and everything
in it shipped here.

### Added

- The editor can be driven with a mouse: a toolbar, a right-click menu with the clipboard verbs in
  it, a settings panel whose rows change on a click, and a first-run wizard whose cards can be
  clicked. That last one mattered most — the screen asking whether you would like the conventional
  mouse-shaped editor could only be answered with the arrow keys.
- Three settings, which brings the panel back to covering every option that exists: `autosave`,
  which writes a moment after typing *stops* rather than on a timer, so what reaches the disk is a
  state you stopped at; `line_numbers`, absolute by default without Vim keys, because relative
  numbering exists so `12j` can be read off the screen; and save as, on ctrl-shift-s.
- The search box steps through matches with enter and back with shift-enter, leaving the box open,
  and hands its query to a project-wide search with ctrl-shift-f — which is what you want the moment
  it turns out not to be in this file.

### Changed

- Starting with no argument reopens the last project and its session rather than the working
  directory, which for a Start Menu entry or a desktop icon is wherever the launcher happened to
  point. On Windows that was the install directory, which is nobody's project.
- With Vim keys off, the chrome no longer reports a mode it does not have. The title bar announced
  "INSERT MODE" in the largest label on screen and now says what language the file is, and the hint
  line advertises F1 and the command palette instead of `i / v / R / esc mode`, which taught four
  keys that do nothing to precisely the person who had turned them off.
- The search box says `3 of 17` rather than `17 matches`, and names its three keys while it is open.

### Fixed

- Every ctrl-shift binding works again. Window systems apply shift to the character before it
  reaches the editor, so ctrl-shift-p arrived as `P` and every binding written for a lowercase
  letter never matched: the command palette, delete-line, select-all-occurrences, redo and project
  search were all dead in 0.2.0, advertised in the help panel and the menus the whole time.
- `↵` drew as a tofu box in the palette footer — the bundled font has the arrows but not the return
  symbol. It is spelled out in words now.

## [0.2.0] - 2026-08-14

### Added

- The non-modal mode is a conventional editor rather than the modal one with its modes removed: line
  moves, duplication, comment toggle, occurrence selection, word deletion, and a mouse that selects
  words, lines and extra cursors. Vim reaches those by *composition* (`ddp`, `yyp`), and with the
  grammar off there is nothing to compose with.
- Four ways to find out what the editor can do, all reading one table so they cannot disagree: F1
  lists the keys, ctrl-shift-P every command, and there is a menu bar and a context menu. Every one
  of them prints the shortcut for what it offers, so the mouse path teaches the keyboard rather than
  replacing it.

### Changed

- Copy and paste use the system clipboard, which they did not before.

### Fixed

- The Windows title bar showed `crates\vex\src\main.rs` as a single breadcrumb, because the path was
  split on `/` alone.

## [0.1.1] - 2026-08-14

The first Windows run, by someone other than CI, found three things no test on Linux could have
seen.

### Added

- A real Windows installer. The 0.1.0 artifact was a bare `vex.exe`, which behaves exactly as it
  looks and opens the editor instead of installing it. `vex-setup.exe` is per-user, so there is no
  UAC prompt, and brings a Start Menu entry, `PATH`, "Open with vex" on a folder's context menu, and
  an uninstaller that takes the `PATH` entry back out and leaves `%APPDATA%\vex` alone.

### Fixed

- Built-in theme names load again. The settings panel writes `theme = "amoled"` into `init.lua` and
  the config reader looked a theme name up only as a file in `themes/`, so the panel's own output
  was rejected by the reader that wrote it and every built-in was unreachable by name. The panel
  also used to write `"dark"` when the loaded theme came from a file, throwing away an imported
  theme the next time anyone nudged the font size.
- The folder chooser can leave the folder it opened in. `BrowseForFolder`'s fourth argument is the
  *root* of the tree it shows rather than where it starts, so passing the current project's parent
  put everything above it out of reach.
- Helper processes no longer flash console windows. A release build has no console of its own, so
  Windows handed every console child — git on each diff reload, the language server, the chooser's
  PowerShell — a brand new console window in front of the editor.
- A release build on Windows is a GUI application, so launching it from the Start Menu no longer
  opens a black console window behind the editor. Debug builds keep their console, which is where
  diagnostics go.
- `file://` URIs on Windows, which broke *every* language-server feature there — diagnostics, hover,
  completion, rename, formatting. vex percent-encoded the whole path, so it sent
  `file://%5C%5C%3F%5CC%3A%5C...` and rust-analyzer rejected every request as an "invalid
  international domain name", because `file://` followed by anything other than a slash is parsed as
  a host.
- A config file may start with a byte-order mark. Notepad writes one and so does PowerShell's
  `Set-Content`, and Lua rejected the whole file for it with "unexpected symbol near `'<\239>'`" —
  naming a byte the user cannot see, in a file they will swear is correct. The TOML reader failed
  more quietly still: the mark became part of the first section's name, so everything above the
  second heading was simply never read.

## [0.1.0] - 2026-08-13

First packaged release. A modal editor with Vim's grammar, tree-sitter highlighting and
language-server support, drawn directly on the GPU with wgpu.

### Added

- Vim's grammar: counts, operators, motions, text objects, registers, macros and a jumplist. The Vim
  keys can be turned off for people who would rather have a conventional editor, which is the first
  thing the first-run wizard asks.
- Language-server support: hover, go to definition, references, rename, formatting, code actions,
  inlay hints, diagnostic underlines and a diagnostics list, with a server kept per language rather
  than one at a time.
- Tree-sitter highlighting on a worker thread, folding, sticky scroll, soft wrap, auto-pairing,
  indentation and snippet expansion.
- Window splits, a file tree, file watching, project-wide search and replace, a git diff gutter,
  session restore and crash recovery.
- Configuration in Lua with hot reload, a settings panel that covers it without editing a file, five
  built-in themes, and importers for VSCode data, `.vsix` extensions and colour schemes in four
  formats.
- Packaging for deb, rpm, Windows and a portable tarball, with a desktop entry and window icon that
  the desktop actually picks up.

[Unreleased]: https://github.com/Gawad-B/Vex/compare/v0.4.2...HEAD
[0.4.2]: https://github.com/Gawad-B/Vex/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/Gawad-B/Vex/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/Gawad-B/Vex/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Gawad-B/Vex/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Gawad-B/Vex/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Gawad-B/Vex/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Gawad-B/Vex/releases/tag/v0.1.0

# Changelog

Every release of vex, newest first, written for someone deciding whether to upgrade: what changed
and — where it is not obvious — what the old behaviour cost.

The format is [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html). The long-form account of a
release, where one exists, lives in `packaging/notes/`.

## [Unreleased]

### Changed

- A new interface, shaped like VSCode and Zed: a title bar with the logo, a menu bar, a search box
  that opens the command palette and the notifications bell; an icon column down the left that
  switches the sidebar between the explorer, search, source control, run and debug, and problems,
  with settings opening as a tab beside your files; the explorer on the left with file icons and a
  New button; tabs with icons, dots for unsaved files and close buttons, between back/forward and
  split/run buttons; and a status bar that counts errors, warnings and notes with their icons. Every
  button shows its name and shortcut when the pointer rests on it.
- A new logo: the letter ض with its fatha and an amber block, in the title bar beside a wordmark
  with "Da" in amber, on the welcome and empty screens, and as the application icon on every
  platform. Below 64 pixels the two blocks become one, as the logo sheet specifies, and on a light
  theme the letter turns dark while the amber stays.
- One font: everything is set in Fira Code. The serif headings and sans labels, and the two fonts
  they came from, are gone.
- A new default theme, `dacode`, with the chrome a shade darker than the code. The other five are
  still there.
- Switching between light and dark keeps the theme's family where it has one: `coral-dark` and
  `coral-light` swap with each other rather than with the plain `light` and `dark`, and switching
  back returns to the theme you started from.
- The welcome screen's theme cards wrap into rows, dark themes first, are as wide as the longest name
  where the window allows, and shrink to fit a short window; ↑ ↓ move a row. Thirteen cards in one
  row left each too narrow for its name.
- A theme file that does not say whether it is dark or light is classified by its background, and
  the colours it leaves out come from the matching built-in rather than always from the dark one.
- With Vim keys off, shortcuts are written the usual way — `Ctrl+Shift+P` rather than `<c-s-p>` — in
  the menus, the palette, tooltips and messages.

- **A menu bar in the title bar**: File, Edit, Selection, View, Go, Run, Terminal and Help sit after
  the wordmark all the time, where before they existed only while a menu was open. Click a title to
  open its menu; once one is open, moving across the bar opens its neighbours. <kbd>F10</kbd> and
  the arrow keys still reach every menu. A narrow window drops titles from the end behind a "…",
  which lists the hidden ones, and below about 880 pixels the wordmark goes first; a window that is
  widened never loses a title it had.
- **The account icon and the Share button are gone.** The first did nothing and the second spent the
  brand colour on something rarely done. Share is now **File → Copy link to line**, and is still a
  command in the palette. Under Terminal, `run` is now labelled "copy the run command", since that
  is all it does until there is a terminal.
- **The interface moves, expressively.** Popups, menus, the palette and the help panel fade in and
  grow from 0.96 towards full size about the thing that opened them, and fade out again; the
  sidebar slides out from the activity bar; the active tab's background and the activity marker glide
  to where you went; a new tab grows in; the bell swings when a message arrives; and the wizard's
  steps and the settings pages cross-fade. Everything is a spring with a little overshoot, about 300
  ms for panels. Typing, the cursor, selections and scrolling that follows the cursor never animate,
  because an editor that animates keystrokes feels slow.
- **Every overlay is one card**: the palette, the menus and the context menu, completion, the hover
  and diagnostic box, which-key, the help panel and the bell's log share a rounded panel with a soft
  shadow and a 1 px outline, and keys are drawn as small chips. The palette now drops from the
  search box in the title bar, with its commands grouped under headings until you start typing.
  **Rows in the palette and the help panel can be clicked**, and in the help panel Enter runs the
  highlighted key's command.
- **Settings are a tab, not a sidebar panel.** The gear, <kbd>ctrl</kbd>+<kbd>,</kbd> and the
  palette's "settings" open it beside your files, and <kbd>ctrl</kbd>+<kbd>W</kbd> closes it. A
  category tree — Appearance, Editor, Files, Keys, Languages, Import — runs down the left, folding
  into one button when the page is narrow, and the settings are on the right, each with its name,
  a sentence or two underneath where the name does not say everything, and its control. A dot in the
  margin marks a setting that differs from its default, with a ⟲ beside it to put it back. A search
  box at the top filters every page at once and lights the letters that matched; the Keys page lists
  every command with its shortcut. The captions that said `INIT.LUA` or `IMPORTED` are gone.
- **Settings save themselves.** There is no save button and <kbd>ctrl</kbd>+<kbd>S</kbd> no longer
  applies to the page: a change takes effect at once and is written to `init.lua` 600 ms after the
  last one, and again when you quit. A failed write says so on the status line and in the bell. The
  tab writes only the settings you changed on it, into its marked block, so zooming with
  <kbd>ctrl</kbd>+<kbd>+</kbd>, toggling the theme with a key, or importing from VSCode are not
  quietly pinned into your file the first time you touch anything. Resetting a setting removes its
  line rather than writing today's default, so a later import or a change to DaCode's default still
  reaches you. If you edit the setting in `init.lua` yourself while the page's change is still
  waiting, your edit wins. If `init.lua` is open with unsaved edits, the write waits until you save
  or close it, and says so.
- **Keys go where you clicked.** With the file tree open, the arrow keys used to move through files
  whatever you had just clicked, so ↑ and ↓ could not move the caret. Focus is now one of the text,
  the sidebar and the results panel, a click sets it, and the keys follow it. Opening a file from the
  tree hands the keys back to the text, as VSCode does. The tree's selected row is bright with a ring
  while it has the keys and dims when it does not.

- vex is now **DaCode**. The command is `dacode`, the configuration lives in
  `~/.config/dacode/init.lua` (`%APPDATA%\dacode` on Windows), and the Lua table is `dacode`. Nothing
  needs doing by hand: on first start DaCode copies the vex configuration and state directories —
  settings, themes, sessions and any crash-recovery copies — and leaves the originals where they
  were. An `init.lua` that says `vex.set` still runs, `VEX_CONFIG_DIR` and `VEX_STATE_DIR` are still
  read, and the Windows installer upgrades a vex install in place rather than adding a second one.

### Added

- Seven themes from Quizzy: `quizzy-dark` and `quizzy-light`, `coral-dark` and `coral-light`
  (Charcoal coral), `sky-dark` and `sky-light` (Abyss sky), and `amoled-black`. Their surfaces, text
  and accent are Quizzy's own colours; their syntax colours, which Quizzy does not have, are derived
  from each palette and read at 4.5:1 or better against the background. `amoled` is still there,
  under its own name.
- The settings tab's theme row opens a list instead of stepping to the next theme: every theme, under
  **Dark** and **Light**, with its colours beside its name and the one in use marked. Click or press
  enter or space to open it, ↑ ↓ to move, enter or a click to pick, esc or a click outside to close.
  It scrolls in a small window. ← → on the row still step through the themes, in the list's order.
  The list, the row and the welcome screen show each theme by name — *Charcoal coral*, *Rosé Pine*
  — rather than by the slug `init.lua` uses.
- `--theme-dropdown` renders the settings tab with that list open.

- `transparency` — how much of the desktop shows through the window, from `0` to `90` percent, in
  `init.lua` or the settings tab. Backgrounds go see-through; text, highlights and popups stay
  solid so the editor stays readable. It needs a compositor, and says so when there is none.
- **File → Copy link to line** copies a permanent link to the cursor's line — on GitHub, GitLab,
  Bitbucket, Codeberg and the like, pinned to the checked-out commit — or `path:line` when the
  project has no remote.
- The bell keeps the editor's recent messages, so one that vanished at the next keystroke can still
  be read.
- Run copies the command a project of its kind is run with (`cargo run`, `npm run dev`,
  `flutter run`…). There is no terminal to run it in yet, and it says so.
- `animations` — `"full"`, `"reduced"` or `"off"`, in `init.lua` or the Appearance page. `reduced`
  keeps every fade and removes everything that travels or overshoots: the slides, the scale, the
  lift and the bell's wobble. `off` makes every change jump. Switching to `off` finishes whatever is
  in flight at once, and a misspelling such as `"none"` is reported rather than read as `"full"`.
- <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>E</kbd> moves the keys to the file tree, opening it if it is
  closed, and <kbd>ctrl</kbd>+<kbd>1</kbd> moves them back to the text. <kbd>Esc</kbd> in the tree
  also hands them back. Both are in the command palette and the F1 key list.
- Pinch zooms the code where the window system delivers a pinch gesture, which is macOS today, and
  <kbd>ctrl</kbd>+scroll zooms everywhere else, including Wayland, one font-size step per line
  scrolled.
- `--at-ms`, `--scroll-px`, `--menu-title`, `--settings-page` and `--settings-search` for the
  screenshot renderer: any moment of a transition, a view part-way into a row, the menu bar open at
  a title, and the settings tab at a page or with a search typed.

### Fixed

- A touchpad scrolls. A touchpad reports scrolling as a stream of deltas a few pixels each, and each
  one was divided by forty and truncated to nothing, so the code did not move at all. Scrolling now
  follows the pixel: whole rows move the view and the part of a row left over is drawn as the top
  line slid partly out of sight, so the text follows your fingers one to one. A
  notch of a mouse wheel glides three rows instead of jumping them. Ctrl-scroll no longer runs the
  font from the smallest size to the largest in one flick, and holding ctrl to zoom no longer throws
  a scrolled view back to the cursor.
- Syntax colours arrive without a keypress. The highlighter's answer sat unread until some unrelated
  key or click woke the editor, so after opening a file, scrolling or editing, the colours were a
  step behind. Every background worker — the highlighter, git, the formatter, the installer and the
  language-server reader — now wakes the editor when it has an answer.
- An idle editor really sleeps. While a language server was running the loop woke every 100 ms to
  ask whether anything had happened, which is a core never quite asleep and, on a laptop, a battery
  never quite untouched. An idle editor polls nothing on a timer now: what has a deadline, such as an
  autosave, sleeps until it, and the workers wake the loop themselves. Two producers still tick it
  every 100 ms while they run, because neither has a waker yet: the file walk and a project search
  that is still streaming hits. The loop also stops asking for frames while
  the window is minimised.
- Keys no longer go to a panel that is merely open: with the file tree showing, ↓ moved through
  files instead of lines, and an open search took Enter and the arrows whatever you had clicked.
- Choosing "command palette" from a menu no longer prints the closing menu through the palette's
  card, and a menu opened by a click no longer lets the completion popup behind it take Down and
  Enter into your text.
- Saving can no longer destroy the file. Every save used to truncate the file and then write it, so
  a crash, a power cut or a full disk in between left it empty — and the crash-recovery copy had
  already been deleted. Files are now written to a temporary beside them, flushed to the disk, and
  renamed into place, keeping their permissions and owner. A symbolic link stays a link, and a
  hard-linked file is still written in place so its other names see the change.
- `ctrl-s` saves the same way `:w` does. It went through a separate, older save that skipped
  clearing the recovery copy, so work you had saved was offered back to you after a crash, and it
  never ran `format_on_save`.
- A save no longer silently overwrites a change another program made since you opened the file. The
  first save is refused, with a message saying so; saving again overwrites. Autosave never
  overwrites.
- A panic loses no unsaved work. Every unsaved buffer is written to the recovery directory the
  moment the editor panics, where before up to two seconds of typing, and more in a buffer you had
  switched away from, was lost. A kill or a power cut still loses up to the last two seconds.
- Saving no longer makes the project re-index itself. A save-by-rename looks like a new file to the
  file watcher, which would have started a full walk after every save.
- The session file and `init.lua` are written the same safe way, so a crash on the way out cannot
  leave the next start without its open files or its settings.

## [0.4.3] - 2026-08-16

A typographic release, and it came from readers rather than from use: enough people said the same
thing about the code face that being defensible stopped being the point.

### Changed

- The code face is **Fira Code**, bundled, replacing IBM Plex Mono. The design names Plex and it is
  a good face, but the code font is the one thing somebody looks at for hours: an unfamiliar one is
  read as a mistake before it is read as a choice. Only the code face moved — Instrument Serif still
  draws the wordmark and the headings.
- Comments are set as code: the same face, upright, at the same size. They were italic serif a size
  larger, which is a real typographic idea and the thing people noticed first and liked least. It
  also could not hold the grid — a proportional face in a monospaced column leaves a commented-out
  line visibly out of step with the one above it. The theme still dims them, which is the
  distinction that was doing the work.
- The landing page's identity panel names the code face it is describing. It still said IBM Plex
  Mono, which was a claim about the editor rather than about the page.

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

[Unreleased]: https://github.com/Gawad-B/Vex/compare/v0.4.3...HEAD
[0.4.3]: https://github.com/Gawad-B/Vex/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/Gawad-B/Vex/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/Gawad-B/Vex/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/Gawad-B/Vex/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Gawad-B/Vex/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Gawad-B/Vex/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Gawad-B/Vex/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Gawad-B/Vex/releases/tag/v0.1.0

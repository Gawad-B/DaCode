# Command line reference

Every flag and every environment variable, derived from `crates/dacode/src/main.rs`,
`crates/dacode/src/import_cli.rs`, `crates/dacode/src/paths.rs` and the crates they delegate to.

```
dacode [FILE|DIR] [--no-session] [--no-welcome]
dacode --screenshot OUT.png [RENDERING FLAGS] [FILE]
dacode import [SOURCE] [WHAT...] [--out DIR]
dacode tools list | install NAME...
dacode --help | --version
```

`--version` / `-V` and `--help` / `-h` are checked at the top of `main()` — after the `import`
subcommand is dispatched, so `dacode import --help` still reaches the importer's own help, and before
anything that could open a window. Each prints and exits.
`--help` renders the `FLAGS` table (54 entries of flag, value placeholder, description) rather than
carrying its own prose, so the help text and the flag list cannot disagree — the same one-table
discipline `dacode_input::commands::COMMANDS` follows. **That table is the source of truth for this
document**; everything under "Normal use" and "Headless rendering" below is the same set with the
reasoning attached.

`-h` and `-V` are the **only** single-dash forms accepted. `positional()` skips arguments beginning
with `--` and nothing else, so any other single-dash argument is taken as the file to open: `dacode -x`
opens an empty buffer named `-x`.

An unrecognised long flag is ignored silently by the editor and rejected with a message by
`dacode import`.

**Neither `--help` nor `--version` prints anything on a plain Windows release build.** The binary is
compiled `windows_subsystem = "windows"` so that launching it from the Start Menu does not open a
black console behind the editor, which means the process owns no console of its own. A shell that
pipes or redirects — `dacode --help | more`, `dacode --help > out.txt`, or any PowerShell capture — sees
the text; `cmd.exe` run plainly shows nothing. `dacode import --help` has always behaved this way.
Attaching the parent console would fix it, needs FFI that cannot be tested from Linux, and is
therefore written down rather than half-fixed. Debug builds keep their console and print normally.

## `dacode --help`

Verbatim, from a Windows build. The configuration path in the penultimate paragraph is the only
part that varies: it reads `~/.config/dacode` everywhere else.

```
dacode 1.2.0 — a code editor that never stalls on your keystroke

USAGE
    dacode [FILE]              open a file
    dacode [FOLDER]            open a folder as the project
    dacode                     reopen the last project
    dacode import …            bring settings over from VSCode (`dacode import --help`)
    dacode tools …             download formatters and servers (`dacode tools list`)

OPTIONS
    --no-session                start empty rather than reopening the last session
    --no-welcome                skip the first-run questions
    --version                   print the version and exit
    --help                      print this and exit

RENDERING WITHOUT A WINDOW
    Renders one frame to a PNG and exits. This is how the design is checked in CI and
    over SSH. Everything below needs --screenshot.

    --screenshot PNG            render one frame to a file and exit
    --size WxH                  the window to render at (default 1600x1000)
    --light                     the light theme
    --dark                      the dark theme
    --theme NAME                any built-in or installed theme
    --mode NAME                 normal, insert, visual …
    --no-vim                    render with Vim keys off
    --transparency PERCENT      see-through, over a checkerboard
    --zoom PERCENT              at an interface zoom, as ctrl-= sets it
    --empty                     no file open
    --splash                    the cold-start screen
    --at LINE                   put the cursor on a line
    --at-column N               and a column
    --scroll-px N               scroll N more pixels, part-way into a row
    --at-ms N                   draw the moment N ms into whatever is animating
    --columns N                 wrap width, in columns
    --no-wrap                   soft wrap off
    --split N[h]                N panes, `h` for stacked
    --tree                      the file tree
    --minimap                   with the minimap on
    --settings                  the settings tab
    --settings-page NAME        open the settings tab at a page: appearance, editor, keys …
    --settings-search QUERY     type a search into the settings tab
    --row N                     select a settings row, scrolling to it
    --capture-key NAME[=KEYS]   a Keys page row waiting for a key, KEYS pressed into it
    --theme-dropdown            the settings tab with its theme list open
    --welcome STEP              the first-run wizard at one step
    --folded                    with a block folded
    --git                       the diff gutter
    --blame                     the blame column and the caret line's blame
    --palette QUERY             the file palette, filtered
    --commands                  the command palette
    --search QUERY              the find box
    --grep QUERY                the search view, with its results
    --symbols QUERY             the symbol picker
    --complete PREFIX           the completion popup
    --signature LABEL           the signature card; *stars* mark the lit argument
    --diagnostic L:C:MSG        one diagnostic, inline
    --diagnostics-list          the problems list
    --source-control            the source control view, over a canned status
    --no-repository             the source control view in a folder git does not track
    --diff-view                 a diff tab, showing a canned diff
    --branches                  the branch picker, over canned branches
    --terminal                  the terminal panel, showing a fixed transcript
    --terminal-tabs N           with --terminal: that many terminal tabs
    --terminal-height PERCENT   with --terminal: the panel dragged to that share
    --terminal-select R:C-R:C   with --terminal: a mouse selection
    --help-panel                the F1 key list
    --update-offer KIND         the bell offering an update: ready, available or package
    --menu                      the menu bar
    --menu-title TITLE          the menu bar open at a title, or `...`
    --context-menu              a right-click menu
    --which-key PREFIX          what can follow a half-typed chord
    --block LINE:COLUMN         a rectangular selection to that corner

Configuration lives in %APPDATA%\dacode. DACODE_CONFIG_DIR and DACODE_STATE_DIR move it;
DACODE_NO_SYSTEM_FONTS=1 makes a render reproducible.

Keys: press F1 in the editor, or read CHEATSHEET.md.
Docs: https://github.com/Gawad-B/DaCode/tree/main/docs
```

Column widths are computed from the longest entry, so adding a flag cannot leave them ragged. The
version comes from `CARGO_PKG_VERSION` and a test asserts the help text carries it, because
`dacode --version` is what a bug report is asked for.

`FLAGS` is split at `PLAIN_FLAGS` (4) into the two groups above. The headless group is long because
it is the whole visual test surface, and someone reaching for it is reproducing a capture from CI
rather than editing a file.

## Normal use

The first argument that is not a flag — and is not the value belonging to one — is what to open.

| Argument | Takes | What it does |
|---|---|---|
| *(positional)* `FILE` | — | Opens the file. A path that does not exist yet becomes a named empty buffer, so `dacode newfile.rs` works. An image, a PDF or an office document opens in the viewer ([Viewing files](../guides/viewing-files.md)). Any other file that is too large, binary or not UTF-8 is refused with a sentence and the buffer keeps no path, so it can never be saved over what it failed to read. |
| *(positional)* `DIR` | — | Opens the directory as the project root and shows the file tree. The directory itself becomes the root rather than searching upwards for a `.git`, because naming a folder is an explicit choice about what the project is. |
| `--no-session` | — | Do not restore the previous session, and do not fall back to the last project when started with no argument. |
| `--no-welcome` | — | Skip the first-run wizard even when no configuration exists. |
| `--version`, `-V` | — | Print `dacode 0.4.3` and exit. |
| `--help`, `-h` | — | Print the usage text and exit. |

With no argument at all, DaCode reopens the last session for the last project worked in — not whatever
directory the launcher happened to point at, which on Windows is where the program was installed.
Naming a file explicitly wins over the session: asking for one file and getting the last three back
would be surprising.

`--no-vim` used to be listed in this group, described as "start with Vim keys off, whatever the
config says". It was only ever read inside `screenshot()`, so outside `--screenshot` it did nothing —
the editor's starting mode comes from `Settings::modal`. It is a headless flag now, and the guard
`a_flag_offered_for_ordinary_use_works_without_screenshot` fails the build if a flag listed for
ordinary use is read only by the renderer again.

### `VALUE_FLAGS`

A flag that consumes the argument after it must be named in `VALUE_FLAGS`, or `positional()` takes
that argument as the file to open. The list holds twenty-four names.

It has been wrong twice. `--columns 60` once tried to open a file called `60`, which is what the
list was added for; then `--row`, `--symbols` and `--block` were added to the renderer over the
following months and none was added here, so `--row 12` opened an empty buffer called `12` behind
the settings panel it was meant to be scrolling. Nothing failed — the panel still drew — which is
why rendering every overlay in CI could not see it either.

`command_line_tests` in `main.rs` now derives the flag set from the source instead of trusting
anyone to remember, and checks:

- every flag whose value is consumed is in `VALUE_FLAGS`
- every flag `FLAGS` offers is really read by the binary
- every flag really read by the binary is in `FLAGS`
- every flag `FLAGS` lists for ordinary use is read outside `screenshot`
- every flag `FLAGS` lists carries a description

It strips comment lines first and matches only the three shapes a flag is actually read in — an
equality against an argument, and the two closures `screenshot` builds over its argument list —
because a doc comment naming a flag is not the binary reading one. The first three were verified by
injecting violations, and another test asserts the scan still finds three flags read three
different ways, so a scan that silently stopped matching would fail rather than pass vacuously.

## Headless rendering

`--screenshot OUT.png` renders one frame offscreen and exits. It needs no window server, which is how
the design is checked in CI and over SSH — and it is reproducible in a way a screen capture is not.
Set `DACODE_NO_SYSTEM_FONTS=1` for every capture: it makes the font set deterministic and startup
faster.

With no file named, the capture opens `crates/dacode-text/src/anchor.rs`, a real file in this repo, so
the shot shows real code with real highlighting rather than a fixture that could hide a defect. The
cursor is parked at line 21, column 12, the view scrolled to line 8, and in normal mode a half-typed
`3d` is seeded into the pending cell.

### Surface and appearance

| Flag | Takes | What it does |
|---|---|---|
| `--screenshot` | `PATH` | Required. The PNG to write. |
| `--size` | `WxH` | Render at that size. Default `1600x1000`; each dimension is floored at 200. Split on `x` or `X`. |
| `--light` | — | Render the light built-in, overriding config. |
| `--dark` | — | Render the dark built-in. Only consulted if `--light` is absent. |
| `--theme` | `NAME` | Render a named built-in. Only consulted if neither `--light` nor `--dark` is given. An unknown name falls back to config. |
| `--mode` | `NAME` | `insert`, `visual` or `replace`. Anything else, including a missing flag, is normal mode. Visual mode also selects 24 characters from the cursor. |
| `--no-vim` | — | Modal editing off: insert mode, and the seeded `3d` is cleared, since there is no operator-count to be half-way through without the grammar. |
| `--zoom` | `PERCENT` | Draw at an interface zoom, as `ctrl-=` sets it. |
| `--minimap` | — | Draw with the minimap on, whatever config says. |
| `--transparency` | `PERCENT` | Draw the window see-through, composited over a checkerboard so the capture shows what the desktop would. |

Capture at more than one size. The harness was fixed at 1600x1000 for a long time and a whole family
of defects lives below it — the settings rows collided with the footer under about 900px of height,
and the toolbar was laid out at a negative x under about 450px of width. Anything with a panel in it
gets captured small too.

### What is on screen

| Flag | Takes | What it does |
|---|---|---|
| `--empty` | — | Open nothing, so the empty state is what is drawn. Overrides a named file. |
| `--columns` | `N` | Force the soft-wrap width to N columns, floored at 8, so wrapping can be inspected without resizing a window. |
| `--no-wrap` | — | Turn soft wrap off. |
| `--at` | `LINE` | Park the cursor on a line, zero-based, clamped to the last line, with the view scrolled two rows above it. Sticky scroll only appears once a block head has actually scrolled off. |
| `--at-column` | `N` | Scroll sideways to column N, turning wrap off. Without it the horizontal axis cannot be captured at all, and an axis whose defects cannot be seen is one whose defects ship. |
| `--scroll-px` | `N` | Scroll on by N pixels after `--at` has placed the view, through the same function a touchpad's deltas go through. Part of a row is drawn as the top row slid partly out of view, which is the one place the code's rows have to be clipped against the tab bar or the pinned strip, so this is how that clipping is looked at. Negative goes back up. Not a whole number of pixels is fine; not a number at all is an error. |
| `--at-ms` | `N` | Draw the frame N milliseconds into whatever is animating. Every animated value is a pure function of the instant it is read at, so the capture moves the clock rather than waiting for it: the result is the same on every machine. `0` is the first instant of a transition. Without it the capture is drawn ten seconds in, which is past where every spring settles, so a plain capture is always the finished screen and never a half-arrived one. Anything that is not a whole number of milliseconds is an error rather than being ignored. What animates: the palette, menus, help, completion, hover and the bell's log as they arrive; the sidebar, the active tab and the activity marker as they move; a new tab; the wizard's steps and the settings pages. Typing, the cursor and selections never do. |
| `--block` | `LINE:COLUMN` | Draw a rectangular selection from the cursor to there — the one gesture that has no key. A missing `:COLUMN` means column 0. |
| `--diagnostic` | `LINE:COL:MESSAGE` | Inject one error diagnostic, twelve columns wide, at that zero-based position. The cursor is parked three lines below so the underline renders unobstructed. Needs all three parts. |
| `--diagnostics-list` | — | Fill the buffer with five diagnostics of both severities across two files, and open the diagnostics sidebar. |
| `--signature` | `LABEL` | Show the signature card at the caret for a call labelled `LABEL`, with the part between two `*`s lit as the argument being typed: `"fn push(&mut self, *value: T*)"`. A canned first line of documentation sits under it. |
| `--complete` | `PREFIX` | Type ` PREFIX` at the end of the cursor's line and open the completion popup over a canned rust-analyzer-shaped list — kinds, types and a deprecation, because a fixture of bare labels cannot show whether the type column fits. |
| `--search` | `QUERY` | Open the search prompt with live matches against the buffer. |
| `--grep` | `QUERY` | Run a real project search and show it in the search view. Waits up to ten seconds; a one-shot render can afford to, the editor never does. |
| `--palette` | `QUERY` | Open the file palette over a real index of this project. Waits up to ten seconds for the walk. |

### Which surface gets drawn

The flags below are an if/else chain: **only the first one present takes effect**, in exactly this
order.

| Order | Flag | Takes | What it draws |
|---|---|---|---|
| 1 | `--splash` | — | The cold-start splash. Ignores the buffer entirely. |
| 2 | `--git` | — | The editor with a canned diff — two added lines, two modified, one removed — so the gutter marks do not depend on what happens to be uncommitted. |
| 2a | `--blame` | — | The editor with the blame column on and a canned blame — three authors in runs down the file, lines 13 and 14 not committed yet — so neither the column nor the caret line's note depends on who last committed to the file on screen. `--at` puts the caret on the line whose note you want to see. |
| 3 | `--folded` | — | Every top-level block folded, so the marker and the hidden-line count are visible. |
| 4 | `--diagnostics-list` | — | The diagnostics sidebar. |
| 4′ | `--source-control` | — | The source-control view over a canned status, read through the real porcelain parser: a branch two ahead and one behind its upstream, a conflict under Merge Changes, a staged rename into a directory with a space in its name, a file both staged and changed again, a deletion, and two untracked files — one with a name too long for the row. A message is typed, and the list has the keys with a file selected, so its row buttons show. |
| 4″ | `--no-repository` | — | The source-control view in a folder git does not track: the explanation and the Initialize Repository button. |
| 4‴ | `--diff-view` | — | A diff tab — the read-only tab a change opens in — showing a canned two-hunk diff, with its added, removed and hunk lines washed in the gutter's colours. |
| 4⁗ | `--branches` | — | The branch picker over canned branches: "create new branch" first, local branches with the current one marked, then remote ones without the remote's `HEAD`. |
| 4a | `--terminal` | — | The terminal panel open under the code, with the keys, showing a fixed transcript fed to the same parser a live shell's output goes through — a prompt, a coloured `ls`, a build, `git status`, every attribute the panel draws, the sixteen colours as swatches, 256-colour and 24-bit text, and the cursor waiting at a prompt. No shell is started, so the capture is the same on every machine. `--terminal-tabs N` shows that many tabs in its header (the first is the transcript's), `--terminal-height PERCENT` the panel as if its edge had been dragged to that share of the editor — clamped as a drag is — and `--terminal-select R:C-R:C` a mouse selection from the first cell to the last, rows and columns counted from zero on the screen. |
| 5 | `--welcome` | `STEP` | The first-run wizard. `keys` is the first screen, `theme` the second with the third option highlighted, `import` the third. Any other value lands on `theme` without the highlight. Ignores the buffer. |
| 6 | `--help-panel` | — | The F1 key list. |
| 6a | `--update-offer` | `KIND` | The bell's panel open over a canned DaCode 1.2.0: `ready` (the default) offers **Restart now**, `available` the **Update** a `notify` setting waits on, and `package` the `sudo dnf install` command and **Download** an `.rpm` install is given. Nothing is fetched. |
| 7 | `--menu` | — | The menu bar, opened. |
| 7a | `--menu-title` | `TITLE` | The menu bar open at one title, named as on the bar (`Help`). A title the window is too narrow to show opens as F10 reaches it, under "…" with its name as a header; `...` opens the overflow's list of hidden titles. |
| 8 | `--context-menu` | — | A right-click menu at (420, 300). |
| 9 | `--which-key` | `PREFIX` | The which-key popup, driven by the pending cell exactly as a half-typed chord drives it. Quote it: `--which-key '<c-k>'`. |
| 10 | `--symbols` | `QUERY` | The symbol palette over a canned ten-entry outline, so its indenting and kind column are inspectable without a language server. |
| 11 | `--commands` | — | The command palette listing every command. |
| 12 | `--settings` | — | The settings tab, with the Languages page's servers and formatters detected against this machine — the page is a report, and a screenshot of a report with invented contents verifies nothing. |
| 12a | `--settings-page` | `NAME` | The page the tab opens at, by its name in the tree: `appearance`, `editor`, `files`, `keys`, `languages`, `import`. Implies `--settings`. |
| 12b | `--settings-search` | `QUERY` | A search typed into the tab, so its results from every page can be captured. Implies `--settings`. |
| 12c | `--row` | `N` | Which entry of the page is selected, headings counted, as ↓ would select it. Implies `--settings`; the page scrolls to its selection, which is the only way to capture a row below the fold. |
| 12c′ | `--capture-key` | `NAME[=KEYS]` | The Keys page with the row of command `NAME` waiting for its new key, as a click leaves it. `=KEYS`, in config notation, presses those keys into it through the page's own capture: a key another command has leaves the row asking to confirm (`toggle_comment=<c-w>`), any other moves it and leaves the row with its ⟲ (`toggle_comment=<c-a-m>`). Implies `--settings`. |
| 12d | `--theme-dropdown` | — | The Appearance page with the theme row's dropdown open on the theme in use, scrolled to it — combine with `--size 640x480` to see the list scroll. Implies `--settings`. |
| 13 | `--tree` | — | The file tree, revealing `crates/dacode-text/src/anchor.rs`. Indexes inline, waiting up to two seconds. |
| 14 | `--split` | `N[h]` | N panes on one buffer, clamped to 1–4, defaulting to 2 if unparseable. A trailing `h` stacks them horizontally; anything else is vertical. Each pane looks at a different place in the file, because identical panes would hide a scroll bug. |

Anything that only *modifies state* — `--palette`, `--search`, `--grep`, `--complete`,
`--diagnostic`, `--at`, `--at-column`, `--scroll-px`, `--block`, `--columns`, `--no-wrap` — needs the chain to fall
all the way through to the plain editor to be visible. Combining `--palette` with `--splash` shows
the splash and nothing else.

## `dacode import`

A separate, explicit operation rather than something that happens at startup, because it rewrites
configuration and its results should be reviewable afterwards. **Nothing is imported unless it is
named.** An importer that quietly replaced your theme because you wanted your keybindings is one
nobody runs twice.

VSCode extensions are JavaScript against the `vscode` API and DaCode has no JS runtime. Extension
*data* — themes, snippets, language configuration — is imported; extension *code* is reported as
unsupported and skipped. Packages come from Open VSX, not Microsoft's marketplace, whose terms permit
only Microsoft products.

### Source — exactly one

| Flag | Takes | What it does |
|---|---|---|
| `--from-vscode` | `[DIR]` | Read a VSCode user directory. The directory is optional: without it, the usual places are searched, and a following argument beginning with `--` is not mistaken for one. |
| `--vsix` | `FILE` | Read a `.vsix` extension package from disk. |
| `--openvsx` | `ID` | Download `publisher.name` from Open VSX into the output directory, then read it. The package is kept, so re-importing later needs no network. |
| `--palette` | `FILE` | Read a colour scheme: `.json`, `.tmTheme`, `.plist`, `.yaml`, `.yml` or `.toml`. Handled before argument parsing and needs no checklist — a colour scheme is one thing rather than a set to choose between. Always writes to the default config directory, ignoring `--out`. |

Omitting a source is an error naming the three that exist.

### What to import — any number

| Flag | What it covers |
|---|---|
| `--settings` | Editor preferences from `settings.json`, written as `imported-settings.toml`. |
| `--keybindings` | Key bindings, where a DaCode equivalent exists, written as `imported-keys.toml`. |
| `--theme` | Colour theme, converted to DaCode's perceptual palette and written to `themes/<slug>.toml`. |
| `--snippets` | Snippet definitions, one file per language under `snippets/`. |
| `--language-config` | Brackets, comment tokens, auto-close pairs. Recognised and reported, not yet consumed. |
| `--language-server` | A language server named by the extension. Recognised and reported. |
| `--all` | Everything the source offers. |

Selecting nothing is an error rather than a silent no-op.

### Other

| Flag | Takes | What it does |
|---|---|---|
| `--list` | — | Show what the source offers and import nothing. For a `.vsix` it marks each capability portable, informational or unsupported. |
| `--out` | `DIR` | Write to DIR instead of the default config directory. |
| `--help`, `-h` | — | Print the help text. Checked before anything else. |

`dacode import` exits non-zero if any selected item failed.

**The default output directory is not the config directory the editor reads.**
`import_cli::default_config_dir` is `$HOME/.config/dacode`, hard-coded: it does not consult
`DACODE_CONFIG_DIR`, and it does not fall back to `USERPROFILE` the way `paths::home` does, so on
Windows without `HOME` it resolves relative to the working directory. Use `--out` to be sure.

## `dacode tools`

```
dacode tools list
dacode tools install NAME...
```

`list` prints every formatter and language server DaCode can download, its pinned version, and
whether it is installed, available, or has no build for this machine. `install` downloads each
`NAME`, checks it against the SHA-256 DaCode carries for it, unpacks it into the state directory's
`tools/` folder and puts a launcher in `tools/bin`, which every run of DaCode appends to the end of
`PATH`. An npm-based tool first downloads the copy of Node it runs on. Installing one already
installed does nothing. Exits non-zero if any `NAME` failed or is unknown. See
[Language servers](../guides/language-servers.md#installing-one-from-inside-the-editor).

## Environment variables

Presence-only variables are checked with `var_os(...).is_some()`, so any value including an empty
one turns them on. The directory variables treat an empty value as unset, because `APPDATA=` is how
a stripped environment presents and joining onto it would put the config directory at the filesystem
root.

### DaCode's own

| Variable | Read by | What it changes |
|---|---|---|
| `DACODE_CONFIG_DIR` | `paths::config_dir` | The configuration directory — `init.lua`, `themes/`, `snippets/`, the imported TOML. Overrides the platform default on every platform. Not consulted by `dacode import`'s default output path. |
| `DACODE_STATE_DIR` | `paths::state_dir` | The state directory: sessions and crash-recovery copies. |
| `DACODE_LSP_TRACE` | `dacode_lsp::client`, `main.rs` | Presence only. Logs every message to and from the language server, plus a wake-up counter every twenty event-loop turns and why a reply was dropped. |
| `DACODE_NO_SYSTEM_FONTS` | `dacode-render/src/text.rs` | Presence only. Skips the system font scan, which costs about 150ms and buys fallback coverage the four bundled faces cannot. Use it for every screenshot. |
| `DACODE_FONT_TRACE` | `dacode-render/src/text.rs` | Presence only. Prints how many faces were bundled and loaded from the system, and how long each took. |

### Platform

| Variable | Read by | What it changes |
|---|---|---|
| `HOME` | `paths::home`, `import_cli`, `dacode_import::vscode_config_dir`, the open-folder prompt | The home directory, tried first — it is set and correct under a POSIX shell on Windows too. Also the only source for `dacode import`'s default output directory and for `~/` in the open-folder prompt. |
| `USERPROFILE` | `paths::home`, `dacode_import::vscode_config_dir` | The Windows home directory, used when `HOME` is unset. |
| `APPDATA` | `paths::resolve`, `dacode_import::vscode_config_dir` | The roaming profile. Config goes here on Windows, because it is the user's and follows them. Also where VSCode's own configuration is looked for first. Falling back to the profile root when unset. |
| `LOCALAPPDATA` | `paths::resolve` | The machine-local profile. State goes here on Windows and must not roam: a recovery copy of a file that exists on one machine is meaningless on another. |
| `XDG_CONFIG_HOME` | `paths::resolve` | The config directory on non-Windows platforms, before falling back to `~/.config/dacode`. |
| `XDG_STATE_HOME` | `paths::resolve` | The state directory on non-Windows platforms, before falling back to `~/.local/state/dacode`. |
| `PATH` | `dacode_lsp::resolve`, `resolve_command`, `paths::program_on_path` | Where a language server, formatter or installer is looked for. A configured command containing a separator is treated as a path and never looked up here. |
| `PATHEXT` | the same three | Which extensions count as executable on Windows. Defaulted to `.EXE;.CMD;.BAT;.COM` when unset. Consulting it is what finds the `.cmd` shims most npm-installed language servers arrive as; a bare-name check finds none of them. |
| `LANG` | `dacode-render/src/text.rs` | The locale handed to the shaper, taken as everything before the first `.` and defaulting to `en-US`. |

### Where things end up

| | Unix | Windows |
|---|---|---|
| Config, themes, snippets | `$XDG_CONFIG_HOME/dacode` or `~/.config/dacode` | `%APPDATA%\dacode` |
| Sessions, crash recovery | `$XDG_STATE_HOME/dacode` or `~/.local/state/dacode` | `%LOCALAPPDATA%\dacode` |

`DACODE_CONFIG_DIR` and `DACODE_STATE_DIR` override both columns. Config and state never share a directory
on any platform, and there is a test that says so.

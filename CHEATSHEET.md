# DaCode — every key

Vim's grammar, so the usual composition works: `[count] ["register] operator [count] {motion|object}`.
`3dw`, `d2f)`, `ci"`, `"ayy`, `.` to repeat. Everything below is the vocabulary that composes.

Every entry here is checked against the real grammar by
`cargo run -p dacode-input --example cheatsheet`, which feeds all 120 of them through it and fails if
one stops resolving. That check found `ctrl-p` falling through to `p` and pasting.

---

## If you never want to learn any of this

**You do not have to touch the keyboard at all.** The **menus** — File, Edit, Selection, View, Go
and the rest — are in the title bar along the top, and **F10** opens them from the keyboard; the
search box in the middle of that bar is every command. The **column of icons on the left** opens the
explorer, project search, source control and the problems list, with the settings gear
at its foot. The **row with the tabs** has a home button that opens a folder, back and forward, split
and run. The **bell** at the top right is your notifications. **Right-click** anywhere in the text for
cut, copy, paste, go-to-definition and the rest. Every button does what its shortcut does, and the
settings tab changes with a click.

When you want the keys: press **F1** for the ones that work right now, in the grammar you are
actually using, filterable by typing. **ctrl-shift-P** is every command, searchable, each with its
key beside it — use it twice and you will have learned the shortcut. Every menu and button prints
its key, so the mouse teaches the keyboard and then gets out of the way.

Nothing below this line is worth memorising until you want it to be.

## With Vim keys off

Set `vim_keys = false` (settings tab, Editor page, first row). There are no modes: typing inserts, and these are
the keys — the ones you already know, because muscle memory is not something an editor gets to have
opinions about.

| Key | Does |
|---|---|
| `ctrl-c` `ctrl-x` `ctrl-v` | copy · cut · paste — through the **system** clipboard |
| `ctrl-z` `ctrl-y` / `ctrl-shift-z` | undo · redo |
| `ctrl-a` `ctrl-l` | select all · select this line |
| `ctrl-d` | select the word, then the next copy of it |
| `ctrl-shift-l` | select every copy of it |
| `alt-f` `alt-t` `alt-a` | select the enclosing function · type · this argument |
| `shift-alt-→` | grow the selection to the enclosing piece of syntax |
| `ctrl-alt-↑` `ctrl-alt-↓` | add a cursor above · below |
| `alt-↑` `alt-↓` | move these lines |
| `shift-alt-↑` `shift-alt-↓` | copy these lines |
| `ctrl-shift-k` | delete this line |
| `ctrl-/` | comment or uncomment |
| `tab` `shift-tab` | indent · outdent (over a multi-line selection) |
| `ctrl-backspace` `ctrl-delete` | delete a word |
| `ctrl-f` `ctrl-shift-f` | find here · find in every file |
| `F3` `shift-F3` | next match · previous |
| `ctrl-s` `ctrl-shift-s` | save · save as |
| `ctrl-q` | quit |
| `F12` `shift-F12` | go to definition · find every use |
| `ctrl-shift-o` `ctrl-t` | a symbol in this file · anywhere in the project |
| `F2` | rename everywhere |
| `F8` `shift-F8` | next problem · previous |
| `F7` `shift-F7` | next change since the last commit · previous |
| `alt-b` `shift-alt-b` | who changed each line, in the gutter · the whole commit for this line |
| `ctrl-space` | suggest a completion |
| `alt-←` `alt-→` | back to where you were · forward again |
| shift + arrows | extend the selection |
| `ctrl-←` `ctrl-→` | by word · `ctrl-home` `ctrl-end` to the file's ends |
| click · drag | place the cursor · select |
| double-click · triple-click | select the word · the line |
| alt-click | add a cursor there (click it again to take it away) |
| alt-drag | select a **rectangle** — one cursor per line, same columns |
| ctrl-click | go to the definition |
| right-click | cut, copy, paste, definition, rename, format — with their keys |

**Never want to press ctrl-s?** Turn on **autosave** in the settings tab (the gear in the icon column): the file is written a moment after you stop typing. It waits for a pause rather
than running on a timer, so what lands on disk is always something you stopped at.

`ctrl-e`, `ctrl-p`, `ctrl-b`, `ctrl-w …`, `ctrl-k …`, `ctrl-,`, `F1`, `F10` and `ctrl-shift-p` are
DaCode's own and work in both modes. Everything below this line describes the Vim grammar, which is on
by default.

## Modes

| Key | Does |
|---|---|
| `i` `a` | insert before / after the cursor |
| `I` `A` | insert at the first non-blank / end of line |
| `o` `O` | open a line below / above |
| `v` | visual (selection) mode |
| `R` | replace mode |
| `esc` | back to normal; in normal mode, collapse to one cursor |

## Motions

| Key | Moves to |
|---|---|
| `h` `j` `k` `l` | left, down, up, right — `space` also moves right |
| `w` `W` | start of the next word / whitespace-delimited word |
| `b` `B` | start of the previous word / WORD |
| `e` `E` | end of word / WORD |
| `0` `^` `$` | line start, first non-blank, line end |
| `{` `}` | previous / next paragraph |
| `%` | the matching bracket |
| `f` `F` `t` `T` + char | find forward / back, and the till variants |
| `G` | last line, or line *count* with a count |
| `gg` | first line, or line *count* with a count |
| `ge` | end of file |
| `gh` `gl` | line start / line end |
| `↑ ↓ ← →` `home` `end` | the same, from any mode |

Vertical motion follows **display** rows, so it moves through soft-wrapped lines as they look.

## Operators

| Key | Does |
|---|---|
| `d` `c` `y` | delete, change, yank |
| `>` `<` | indent, outdent |
| `U` `L` | uppercase, lowercase |
| `x` | delete the character under the cursor |
| `r` + char | replace one character |
| `p` `P` | paste after / before |
| `u` · `ctrl-r` | undo · redo |
| `.` | repeat the last change |

Apply an operator to a motion (`dw`), to itself for the whole line (`dd`), or to a selection in
visual mode.

## Text objects

Prefix `i` for inner, `a` for around.

| Key | Selects |
|---|---|
| `iw` `aw` · `iW` `aW` | word · WORD |
| `i"` `i'` `` i` `` | quoted string |
| `i(` `i)` `ib` | parentheses |
| `i[` `i]` | brackets |
| `i{` `i}` `iB` | braces |
| `i<` `i>` | angle brackets |
| `ip` | paragraph |
| `il` | line |
| `if` `af` | function — its body · the whole of it |
| `it` `at` | type — class, struct, enum, trait, impl, interface |
| `ia` `aa` | argument or parameter — alone · with its comma |
| `ic` `ac` | comment — the words · the whole run of comment lines |

The last four come from the syntax tree, so they work in the languages DaCode highlights: Rust,
Python, JavaScript, TypeScript, C and C++ (HTML and CSS have comments and nothing else to select).
Right after an edit the tree is a moment behind, and they do nothing and say so rather than select
from text that has moved. In visual mode a text object selects itself: `vaf`.

## Multiple cursors

| Key | Does |
|---|---|
| alt-drag | a rectangle — one cursor per line, over the same columns |
| `gA` · `C` | add a cursor above · below |
| `S` | one cursor per selected line |
| `,` | keep only the primary cursor |
| `V` | select the current line (with a count, that many) |
| `g%` | select the whole file |
| `esc` | collapse back to one |

## Search and replace

| Key | Does |
|---|---|
| `/` `?` | search forward / backward, highlighting as you type |
| `ctrl-h` | find and replace in this file — `:replace old new` is the project-wide one |
| `enter` `shift-enter` | next / previous match, **without closing the box** |
| `ctrl-shift-f` | send the same query to a search of every file |
| `n` `N` · `F3` `shift-F3` | next / previous match once the box is closed |
| `ctrl-f` | find in this file · `ctrl-shift-f` in every file |
| `:grep pattern` | search the project for anything |
| `:replace old new` | replace across the files the search found |

The box shows which match you are on — `3 of 17` — and names those keys while it is open, because a
search that counts matches without saying how to visit them reads like a search that cannot.

`esc` closes it. If you only typed, the cursor goes back where it started; if you stepped through
matches with `enter`, you stay where you walked to — being thrown back would be losing your place,
which is the thing returning to the origin exists to prevent.

## Files, buffers and panes

| Key | Does |
|---|---|
| `ctrl-k o` · `ctrl-k ctrl-o` | open a folder — the desktop's folder chooser |
| `ctrl-p` | file palette (fuzzy) |
| `ctrl-b` | buffer palette |
| `ctrl-e` | file tree, shown or hidden — opening it gives it the keys: `j`/`k` move, `enter` opens, `esc` hands the keys back to the text; a click in it or `ctrl-shift-e` gives them to it again |
| `ctrl-shift-e` · `ctrl-1` | keys to the file tree (opening it if closed) · keys back to the text |

**Right-click a row in the tree** for new file, rename and delete. They also live in the command
palette, and act on the tree's selection — or on the open file when the tree is closed. Deleting
asks once and cannot be undone; nothing else here is destructive.

| `ctrl-s` | save |
| `ctrl-h` | find and replace in this file — `tab` between the two fields, `enter` replaces one and moves on, `ctrl-enter` replaces all as one undo |
| `ctrl-f4` | close this file — or click the `x` on its tab |
| `ctrl-q` | quit |
| `ctrl-w v` · `ctrl-w s` | split beside · split below |
| `ctrl-w c` · `ctrl-w q` | close the pane |
| `ctrl-w w` `ctrl-w l` `ctrl-w j` | focus the next pane |
| `ctrl-w W` `ctrl-w h` `ctrl-w k` | focus the previous pane |

Every open file has a tab along the top: click one to switch, click its `x` to close. A dot beside
the name means unsaved changes, and closing a file with unsaved changes warns once — pressing close
again discards them. `ctrl-w` is the pane prefix here, which is why closing a file is `ctrl-f4`.

## Terminal

| Key | Does |
|---|---|
| ``ctrl-` `` | the terminal: open it under the code and give it the keys; from the terminal, hide it and give them back |
| ``ctrl-` `` with the terminal showing | give it the keys, without hiding anything |

The shell is your `$SHELL` (PowerShell on Windows), started in the project root. While the terminal
has the keys **every key is the shell's** — `ctrl-w`, `ctrl-p`, `ctrl-c`, `esc` and the rest mean what
they mean in a shell — except ``ctrl-` ``, `ctrl-shift-v` (paste) and `ctrl-shift-p` (the command
palette). Clicking the code gives the keys back. The wheel over the panel scrolls its history,
`exit` closes it, and its header's `+` and `×` start a fresh shell and hide it. The ▶ button types
the project's run command into it.

## Language server

| Key | Does |
|---|---|
| `K` | hover |
| `gd` | go to definition |
| `gu` | find references → results panel |
| `gs` · `ctrl-shift-o` | go to a symbol in this file — an outline you can search |
| `gS` · `ctrl-t` | go to a symbol anywhere in the project |
| `gr` | rename everywhere |
| `gf` | format the buffer |
| `ga` | code actions → palette |
| `gi` | toggle inlay hints |
| `]d` `[d` | next / previous diagnostic |
| `]c` `[c` · `F7` `shift-F7` | next / previous change since the last commit |
| `]C` | undo this change, back to how it was committed |
| `gb` | who changed each line: a blame column in the gutter (click an entry for its commit) |
| `gB` | the whole commit that last changed this line — hash, author, date, message |
| `gD` | diagnostics list in the sidebar |
| `ctrl-space` | completion (works in insert mode too) |
| `esc` | dismiss a hover or message box |

The completion list shows each suggestion's type beside its name, in the order the **server** ranked
them rather than one DaCode invented. Accepting one inserts what the server asked for — so `println!`
lands as `println!()` with the cursor inside, and `tab` walks any further stops — and brings its
**import** with it: completing `HashMap` in a file without `use std::collections::HashMap` adds that
line too, as one undo step.

## Folding

| Key | Does |
|---|---|
| `za` `zc` `zo` | fold or unfold the block at the cursor |
| `zR` | open every fold |
| `zM` | fold every top-level block |

Folds come from indentation, so they work in every language — including ones with no grammar.

## Jumps and macros

| Key | Does |
|---|---|
| `ctrl-o` `ctrl-i` | back / forward through the jumplist |
| `q` + letter | start recording; `q` again stops |
| `@` + letter | replay that register |
| `@@` | replay the last one used |

Only far-reaching moves enter the jumplist (`gg`, `G`, `}`, `%`, `gd`, `n`, `]d`), so `j` and `w` do
not fill it. Macros record **actions**, not keystrokes, so they survive rebinding a key; a replay is
one undo step.

## Snippets

| Key | Does |
|---|---|
| `tab` (insert mode) | expand the snippet before the cursor, then walk its tab stops |

With no snippet to expand, `tab` indents. Snippets live in `~/.config/dacode/snippets/*.json`, which is
what `dacode import --snippets` writes.

## Settings

| Key | Does |
|---|---|
| `ctrl-,` · `ctrl-k s` | settings tab — or the gear; `ctrl-w` closes it |
| `↑` `↓` · `pgup` `pgdn` · `home` `end` | move between settings |
| `←` `→` · `space` · `enter` | change the selected setting — applies at once and saves itself 600 ms later |
| `delete` | put the selected setting back to its default |
| any other letter · `backspace` | type into the search box — it searches every page — and take a letter back |
| `esc` | clear the search; then leave the tab for the file under it |
| `→` on an import row | tick what to bring over from VSCode; *import now* runs it |
| `enter` on *load a theme file…* | load a colour scheme file — `.json`, `.tmTheme`, `.yaml`, `.toml` |

A dot in the margin marks a setting that differs from its default, and the ⟲ beside it resets it. What
you change is written into a marked block at the end of `init.lua`; your own lines are never
rewritten, and a setting you did not touch is never written.

## Appearance

| Key | Does |
|---|---|
| `zw` | toggle soft wrap |
| `zh` `zl` | with wrap off, look left / right along a long line — 8 columns a press |
| `gt` | toggle light / dark |
| `=` `+` · `-` | larger · smaller font |

## Commands

| Command | Does |
|---|---|
| `:w` · `:w path` | save · save as |
| `:q` `:q!` · `:wq` `:x` | quit · save and quit |
| `:42` | go to line 42 |
| `:e path` | open a file, or switch project if it is a folder |
| `:grep pattern` | search the project |
| `:replace old new` | replace across the search results |
| `:format` · `:rename [name]` | through the language server |
| `:set wrap` / `:set nowrap` | soft wrap on / off |

## Registers and counts

Prefix a yank, delete or paste with `"a` to use register `a`. Prefix anything with a number to repeat
it: `3j`, `2dw`, `d3w` (counts multiply, so `2d3w` deletes six words).

---

## Rebinding

Bindings are consulted before the built-in grammar, but only when no command is half-typed — so
`d<leader>` stays the grammar's business and an operator can never be left dangling by a config file.

```lua
dacode.set { leader = "<space>" }
dacode.map("normal", "<leader>w", "save")
dacode.map("insert", "<c-s>", "save")
```

## Naming a language server

```lua
dacode.server("python", { command = ".venv/bin/pylsp" })
dacode.server("zig", { command = "zls", extensions = { "zig", "zon" } })
```

A **formatter** is named the same way, and `gf` uses it when no language server can format — which
is every plain JavaScript, CSS or Markdown project, and Python, since pyright does not format:

```lua
dacode.formatter("python", { command = ".venv/bin/black", args = { "-" } })
```

Yours is tried before DaCode's own list, which stays behind it as the fallback. A command with a slash
in it is a path — resolved against the project root — rather than a name to look up on `PATH`, and
`~/` is your home directory. A language DaCode already knows (`rust`, `javascript`, `typescript`,
`python`, `cpp`, `go`, `lua`, `dart`) fills in the extensions for you; any other language has to list
them, because a server that can never match a file would otherwise be a line of config that silently
does nothing.
The settings tab's **Languages** page says what was found and where.

In insert mode only modified and named keys can be bound: a letter binding would have to hold the
letter back to see what came next, and typing would stutter.

`dacode.actions` lists every name you can bind to. Print it from your config, or read
`Action::from_name` in `crates/dacode-input/src/action.rs` — they are the same list, and a typo gets a
did-you-mean at startup rather than a key that silently does nothing.

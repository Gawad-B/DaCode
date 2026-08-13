# vex — every key

Vim's grammar, so the usual composition works: `[count] ["register] operator [count] {motion|object}`.
`3dw`, `d2f)`, `ci"`, `"ayy`, `.` to repeat. Everything below is the vocabulary that composes.

Every entry here is checked against the real grammar by
`cargo run -p vex-input --example cheatsheet`, which feeds all 113 of them through it and fails if
one stops resolving. That check found `ctrl-p` falling through to `p` and pasting.

---

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

## Multiple cursors

| Key | Does |
|---|---|
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
| `n` `N` | next / previous match |
| `ctrl-f` | search the project for the word under the cursor |
| `:grep pattern` | search the project for anything |
| `:replace old new` | replace across the files the search found |

`esc` cancels a search and returns the cursor to where it started.

## Files, buffers and panes

| Key | Does |
|---|---|
| `ctrl-p` | file palette (fuzzy) |
| `ctrl-b` | buffer palette |
| `ctrl-e` | file tree — `j`/`k` move, `enter` opens, `esc` closes |
| `ctrl-s` | save |
| `ctrl-q` | quit |
| `ctrl-w v` · `ctrl-w s` | split beside · split below |
| `ctrl-w c` · `ctrl-w q` | close the pane |
| `ctrl-w w` `ctrl-w l` `ctrl-w j` | focus the next pane |
| `ctrl-w W` `ctrl-w h` `ctrl-w k` | focus the previous pane |

## Language server

| Key | Does |
|---|---|
| `K` | hover |
| `gd` | go to definition |
| `gu` | find references → results panel |
| `gr` | rename everywhere |
| `gf` | format the buffer |
| `ga` | code actions → palette |
| `gi` | toggle inlay hints |
| `]d` `[d` | next / previous diagnostic |
| `gD` | diagnostics list in the sidebar |
| `ctrl-space` | completion (works in insert mode too) |
| `esc` | dismiss a hover or message box |

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

With no snippet to expand, `tab` indents. Snippets live in `~/.config/vex/snippets/*.json`, which is
what `vex import --snippets` writes.

## Appearance

| Key | Does |
|---|---|
| `zw` | toggle soft wrap |
| `gt` | toggle light / dark |
| `=` `+` · `-` | larger · smaller font |

## Commands

| Command | Does |
|---|---|
| `:w` · `:w path` | save · save as |
| `:q` `:q!` · `:wq` `:x` | quit · save and quit |
| `:42` | go to line 42 |
| `:e path` | open a file |
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
vex.set { leader = "<space>" }
vex.map("normal", "<leader>w", "save")
vex.map("insert", "<c-s>", "save")
```

In insert mode only modified and named keys can be bound: a letter binding would have to hold the
letter back to see what came next, and typing would stutter.

`vex.actions` lists every name you can bind to. Print it from your config, or read
`Action::from_name` in `crates/vex-input/src/action.rs` — they are the same list, and a typo gets a
did-you-mean at startup rather than a key that silently does nothing.

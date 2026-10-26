# Configuration

How to change DaCode's settings, whether you prefer a page or a file, and what happens when the two
disagree. For the exhaustive list of what you can set, see the
[settings reference](../reference/settings.md).

## The settings tab is the fast path

Open it with the gear in the icon column, with <kbd>ctrl</kbd>+<kbd>,</kbd>, or by typing *settings*
into the command palette. It opens as a tab beside your files, and <kbd>ctrl</kbd>+<kbd>W</kbd>
closes it; pressing it again while it is open just shows it.

A tree of categories runs down the left — **Appearance**, **Editor**, **Files**, **Keys**,
**Languages** and **Import** — and the settings of the chosen one are on the right, grouped under
headings. Each has its name, a sentence or two underneath where the name alone would not tell you what
the choices mean, and its control. On a narrow window the tree folds into one button beside the search
box; click it to step to the next page. A dot in the margin marks a setting that differs from its
default, and the ⟲ beside its control puts it back.

**Search** is the box at the top. Type, and every page is searched at once — names, explanations,
section headings and the config key — with the letters that matched lit. The Keys page is searched
too, so "wrap" finds the soft-wrap command beside the switch.

Click a control to change it, or move with <kbd>↑</kbd><kbd>↓</kbd> and change with
<kbd>←</kbd><kbd>→</kbd>, <kbd>space</kbd> or <kbd>enter</kbd>. <kbd>delete</kbd> resets the selected
setting, any other printable key goes into the search, and <kbd>esc</kbd> clears the search and then
leaves the tab for the file under it.

![The settings tab](../images/settings.png)

The tab also holds the VSCode import checklist and a **load a theme file…** row, so neither of those
needs a terminal, and a report of which language servers and formatters were found. The Keys page lists
every command and its shortcut, but it cannot rebind one: a binding is a line of Lua, so the page has a
row that opens `init.lua`.

### Settings save themselves

There is no save button, and <kbd>ctrl</kbd>+<kbd>S</kbd> on this page just reminds you so. A change
**applies at once**, and is **written to `init.lua` 600 ms after the last change**, so dragging a
setting through five values writes once. Quitting writes anything still waiting. If a write fails —
the file is read-only, say — the status line and the bell say so, and it is not retried until
something else changes.

Three things about that write are worth knowing:

- **Only what you changed on the page is written.** The tab keeps a marked block at the end of
  `init.lua` and puts in it the settings you touched, merged into whatever the block already says.
  Zooming with <kbd>ctrl</kbd>+<kbd>+</kbd>, flipping the theme with a key, or importing from VSCode
  changes the editor you are looking at and writes nothing, so the first setting you touch does not
  pin every other one into your file. Your own lines above the block are never rewritten.
- **A reset removes the setting's line** rather than writing today's default, so a later import, or a
  change to DaCode's default, still reaches you.
- **A hand edit wins.** If you change a setting in `init.lua` yourself while the page's change to the
  same setting is still waiting, the file's value stands and the page's is dropped. The most recent
  intent is the one kept.

**If `init.lua` is open in a tab with unsaved edits**, the tab does not write over them. The write
waits, the status line and the bell say so, and it is made when you save or close that buffer. An
`init.lua` that is open and clean is updated in place, so saving it afterwards does not put the old
block back.

## The file is the durable path

Configuration is a Lua file:

| | |
|---|---|
| Linux, macOS | `~/.config/dacode/init.lua` |
| Windows | `%APPDATA%\dacode\init.lua` |

There is a documented example in the repository. Copy it and edit:

```sh
mkdir -p ~/.config/dacode && cp examples/init.lua ~/.config/dacode/init.lua
```

A minimal file looks like this:

```lua
dacode.set {
  vim_keys = false,
  theme = "rose-pine",
  font_size = 14,
  autosave = true,
}

dacode.map("normal", "<leader>w", "save")
```

**Saving reloads it immediately.** The theme, the metrics and the bindings change in place, and
nothing about your open documents is touched — no restart, and no losing your place.

## Mistakes are reported, not swallowed

A typo in an action name, an unknown theme token or a syntax error produces a box at startup with a
did-you-mean, and **never prevents the editor from opening**. Everything the file managed to set
before the error still applies, so one bad line does not cost you the other twenty.

This is deliberate. An editor that refuses to start because of its own configuration file leaves you
with no editor to fix the configuration file in.

## Which wins

Later beats earlier, and there are three layers:

1. **Built-in defaults.**
2. **An import**, if you ran one — `dacode import --settings` writes into your config.
3. **`init.lua`**, read at startup and again whenever you save it. The settings tab writes into a
   marked block at the end of this file, so its changes are the last word in it, and deleting a line
   from the block hands the setting back to the lines above.

The tab is not a fourth layer: a change made on it is in `init.lua` 600 ms later, and the file is what
the next start reads. Between those 600 ms and the write, a hand edit to the same setting beats it.

The [settings reference](../reference/settings.md) has the exact load order and the precedence rules
for the pieces that are not a simple override — themes in particular, where a file in `themes/` of the
same name as a built-in **shadows** the built-in rather than the other way round.

## Beyond options

`dacode.set` covers the options. Three other functions cover the rest, and all three are documented in
full in the [settings reference](../reference/settings.md):

| | |
|---|---|
| `dacode.map(mode, keys, action)` | bind a key to any command — see [choosing how you type](keymaps.md) |
| `dacode.server(language, spec)` | name a language server — see [language servers](language-servers.md) |
| `dacode.formatter(language, spec)` | name a formatter |

Everything bindable is an `Action`, and the action names are the same ones the command palette shows.
Press <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>P</kbd>, find the command you want, and its name is what
goes in `dacode.map`.

## Trying something without committing to it

```sh
DACODE_CONFIG_DIR=/tmp/experiment dacode
```

runs DaCode against a throwaway configuration directory, leaving yours untouched. `DACODE_STATE_DIR` does
the same for sessions and crash recovery. Both are listed with the rest in the
[command-line reference](../reference/cli.md).

## Where the files actually live

[Files on disk](../reference/files.md) has the full table, including why configuration and state are
kept in different directories — on Windows one roams with your profile and the other must not.

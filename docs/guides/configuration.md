# Configuration

How to change vex's settings, whether you prefer a panel or a file, and what happens when the two
disagree. For the exhaustive list of what you can set, see the
[settings reference](../reference/settings.md).

## The panel is the fast path

Press <kbd>ctrl</kbd>+<kbd>,</kbd>, or click **settings** in the toolbar. Every option is listed with
its current value and — the part a configuration file cannot tell you — **which of the default, your
`init.lua` or an import last set it**. That label under each row is the answer to "why is this on when
I never turned it on".

Click a row to change it, or move with <kbd>↑</kbd><kbd>↓</kbd> and change with
<kbd>←</kbd><kbd>→</kbd>. Changes apply immediately. Nothing is written to disk until you press
<kbd>ctrl</kbd>+<kbd>S</kbd>, which saves the panel's values into your `init.lua`.

![The settings panel](../images/settings.png)

The panel also holds the VSCode import checklist and a **load a theme file…** row, so neither of
those needs a terminal.

## The file is the durable path

Configuration is a Lua file:

| | |
|---|---|
| Linux, macOS | `~/.config/vex/init.lua` |
| Windows | `%APPDATA%\vex\init.lua` |

There is a documented example in the repository. Copy it and edit:

```sh
mkdir -p ~/.config/vex && cp examples/init.lua ~/.config/vex/init.lua
```

A minimal file looks like this:

```lua
vex.set {
  vim_keys = false,
  theme = "rose-pine",
  font_size = 14,
  autosave = true,
}

vex.map("normal", "<leader>w", "save")
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

Later beats earlier, and there are four layers:

1. **Built-in defaults.**
2. **An import**, if you ran one — `vex import --settings` writes into your config.
3. **`init.lua`**, read at startup and again whenever you save it.
4. **The settings panel**, for the current session, until you write it back with
   <kbd>ctrl</kbd>+<kbd>S</kbd>.

The panel's "where did this come from" label tells you which of these last touched each row, which is
usually the question you actually have.

The [settings reference](../reference/settings.md) has the exact load order and the precedence rules
for the pieces that are not a simple override — themes in particular, where a file in `themes/` of the
same name as a built-in **shadows** the built-in rather than the other way round.

## Beyond options

`vex.set` covers the options. Three other functions cover the rest, and all three are documented in
full in the [settings reference](../reference/settings.md):

| | |
|---|---|
| `vex.map(mode, keys, action)` | bind a key to any command — see [choosing how you type](keymaps.md) |
| `vex.server(language, spec)` | name a language server — see [language servers](language-servers.md) |
| `vex.formatter(language, spec)` | name a formatter |

Everything bindable is an `Action`, and the action names are the same ones the command palette shows.
Press <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>P</kbd>, find the command you want, and its name is what
goes in `vex.map`.

## Trying something without committing to it

```sh
VEX_CONFIG_DIR=/tmp/experiment vex
```

runs vex against a throwaway configuration directory, leaving yours untouched. `VEX_STATE_DIR` does
the same for sessions and crash recovery. Both are listed with the rest in the
[command-line reference](../reference/cli.md).

## Where the files actually live

[Files on disk](../reference/files.md) has the full table, including why configuration and state are
kept in different directories — on Windows one roams with your profile and the other must not.

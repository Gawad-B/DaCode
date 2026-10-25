# Changing how it looks

For someone who wants different colours or a different text size. By the end you will have chosen a
theme, know how to bring one over from somewhere else, and know which of your two `theme` settings
wins when they disagree.

## The five that ship

| Name | |
|---|---|
| `dark` | the default |
| `light` | |
| `rose-pine` | |
| `rose-pine-moon` | |
| `amoled` | true black, for an OLED panel |

Cycle them in the settings panel: <kbd>ctrl</kbd>+<kbd>,</kbd>, then the *theme* row, changed with
<kbd>h</kbd>/<kbd>l</kbd>, the arrows, <kbd>enter</kbd> or a click. Each change applies live, so you
are choosing by looking rather than by name. Press <kbd>w</kbd> to write the panel's values into
`init.lua`.

With Vim keys on, `gt` toggles between light and dark. Either way it is in the command palette
(<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd>) as *switch between light and dark*.

From config:

```lua
dacode.set { theme = "amoled" }
```

There is also `appearance = "dark" | "light"`, which flips the built-in theme — but it is a
*fallback* for `theme`, not a companion to it. If you set both, `appearance` does nothing.

With the grammar on, the accent colour follows the mode rather than the theme: four hues at one
shared lightness and chroma, so normal, insert, visual and replace read as one instrument changing
state rather than four themes. That is a property of every theme, built-in or imported, and nothing
downstream is allowed to hardcode a mode hue.

## A file wins over a built-in of the same name

The name in `theme` is resolved in one order, and it is worth knowing which:

1. `themes/<slug>.toml` in your config directory
2. the built-in of that name

**The file is tried first.** A `themes/dark.toml` you wrote therefore shadows the built-in called
`dark` — the more specific answer wins, and silently preferring the compiled-in one would leave you
editing a file that never loads. There is a test named
`a_theme_file_shadows_the_built_in_of_the_same_name`, so this is settled rather than incidental.
(`examples/init.lua` currently describes the opposite; it is wrong, and the
[settings reference](../reference/settings.md#vexset-options) records the drift.)

The name is slugified rather than used verbatim, so `theme = "One Dark Pro"` finds
`one-dark-pro.toml` — which is what the importer wrote it as. A name that matches neither a file nor
a built-in is reported at startup with both the list of built-ins and the path that was tried,
rather than silently falling back.

Where the config directory is: `~/.config/dacode/` on Unix, `%APPDATA%\dacode\` on Windows, or wherever
`DACODE_CONFIG_DIR` points. Themes live in `themes/` under it.

## Bringing a theme over

DaCode reads colour schemes in four formats and converts all of them to its own palette:

| Format | Extensions |
|---|---|
| VSCode JSON | `.json`, `.jsonc` |
| TextMate | `.tmTheme`, `.plist`, `.xml` |
| base16 YAML | `.yaml`, `.yml` |
| DaCode's own TOML | `.toml` |

From the command line:

```sh
dacode import --palette ~/Downloads/gruvbox.yaml
```

That writes `themes/gruvbox.toml`, which you then name like any other:

```lua
dacode.set { theme = "gruvbox" }
```

`--palette` needs no checklist — a colour scheme is one thing rather than a set to choose between.
It always writes to the default config directory, ignoring `--out`.

From inside the editor, the **last row of the settings panel** is *load a theme file…*. Press
<kbd>enter</kbd> on it or click it, and a prompt opens pre-filled with your home directory, since a
downloaded theme is usually in Downloads (`~/` works there too). It reads the same four formats,
applies the result at once, and saves it under `themes/` as DaCode's own TOML — so neither importing a
theme nor choosing one ever needs a terminal.

It also tells you how many colours it recognised and what slug it saved under, and that is worth
reading, because **the panel cannot make that theme stick.** Loading a file changes the running
editor; putting `theme = "<slug>"` in `init.lua` is what survives a restart. The message names the
exact line to write.

To take the theme out of a VSCode extension, ask Open VSX for it by publisher and name:

```sh
dacode import --openvsx zhuangtongfa.material-theme --theme
```

The package is downloaded into the output directory and kept, so re-importing later needs no
network. A `.vsix` you already have works the same way, with `--vsix path/to/it.vsix --theme`. See
[migrating from VSCode](migrating-from-vscode.md) for the rest of what those flags reach.

### What conversion does, and what it does not

Everything is stored in **OKLCH** rather than hex, because a theme's identity is a relationship
between hues and OKLCH is the space in which lightness can change without dragging the hue with it.
Conversion to linear RGB happens where the GPU needs it, not before.

The mapping is deliberately conservative: what is recognised is set, and what is not keeps the
built-in value. A theme is a relationship between colours, and half-inventing the missing half
produces something that belongs to nobody. The importer reports how many colours it actually
recognised, so a scheme that yielded two is one you can see was not really understood, rather than a
mostly-default theme presented as a successful import.

One thing is corrected rather than copied. If the imported text colour and background end up too
close in lightness, the text is re-lightened and the correction is reported — a theme that leaves
text illegible against its own background is worse than no import at all.

**A theme loaded from a file cannot be changed by the panel.** The panel cycles the five built-ins by
name, and a file's colours have no name it could write, so that row shows `from a file` and writes
nothing. Otherwise changing the font size would quietly throw away an imported theme.

## Changing individual colours

`dacode.theme` overrides tokens on top of whichever theme resolved:

```lua
dacode.set { theme = "rose-pine" }

dacode.theme {
  bg      = "#101418",
  comment = "oklch(0.62 0.04 250)",
}
```

Both notations work. OKLCH is the one worth editing by hand, for the same reason the whole palette is
stored in it. Overrides are applied *after* the theme resolves, so they land on top of any base,
including an imported one.

There are nineteen tokens, listed in [the settings reference](../reference/settings.md#vextheme--individual-colours)
and readable from Lua as `dacode.tokens`. They are validated when the config is read rather than when
the theme is painted, so a misspelled name is reported with the spelling you typed and a suggestion,
instead of vanishing.

## Text size

`font_size` is the size of the code face in pixels, and the whole type scale is derived from it —
labels, gutter figures, the serif comments — so changing it scales the interface coherently rather
than making one thing bigger inside a layout built for something else.

```lua
dacode.set { font_size = 15 }
```

Or the `font_size` row in the settings panel. With Vim keys on, `=` and `-` change it directly; in
either keymap it is in the command palette as *bigger text* / *smaller text*.

Two honest notes about the range:

**The effective range is 11 to 18.** Anything outside it is silently brought inside, so
`font_size = 40` renders at 18.

**The panel's own bounds are 8 to 32 and do not match.** So the panel can never actually reach 32,
and a mouse-only user who raises the size to 18 has no way to lower it again from that row. Use
`init.lua` or the palette until that is fixed; it is recorded in the
[settings reference](../reference/settings.md#where-the-three-disagree).

The line height is a fixed multiple of the code size — a generous 1.9. Comments used to be set in
italic serif a size larger, and that ratio is what let the two share a page without the lines
fighting; they are set as code now, upright and in the code face, so the height is simply room to
read in. The gutter and the code area always share a row height exactly; if those ever diverged the
line numbers would stop registering with their lines.

Column advance is estimated from the type scale rather than measured from the shaper. That is why an
elided label can be a character short or long either way — an approximate ellipsis beats an exact
guillotine, but it is an approximation and it is written down as one.

## Checking a theme without a window

Any state renders offscreen to a PNG, which is how the design is checked in CI and over SSH:

```sh
DACODE_NO_SYSTEM_FONTS=1 dacode --screenshot out.png --theme rose-pine
DACODE_NO_SYSTEM_FONTS=1 dacode --screenshot out.png --light --size 900x700
```

Set `DACODE_NO_SYSTEM_FONTS=1` for every capture: it makes the font set deterministic and startup
faster. `--theme` takes a built-in name only, and an unknown one falls back to your config. Every
rendering flag is in the [command line reference](../reference/cli.md#headless-rendering).

## See also

- [Configuration](configuration.md) — the panel, `init.lua`, and which of them wins
- [Settings reference](../reference/settings.md) — `theme`, `appearance`, `font_size`, `dacode.theme`
- [Command line reference](../reference/cli.md#dacode-import) — every `dacode import` flag
- [Migrating from VSCode](migrating-from-vscode.md) — themes as part of a whole setup

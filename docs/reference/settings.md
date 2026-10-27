# Settings reference

Every option DaCode reads, where it is read, and what it does. Derived from
`crates/dacode-lua/src/lib.rs` (the Lua reader), `crates/dacode/src/config.rs` (the settings struct and
its defaults) and `crates/dacode/src/settings_ui.rs` (the settings tab). Where the three disagree, this file
says so rather than picking one.

Config lives in `init.lua` in the config directory — `~/.config/dacode/` on Unix, `%APPDATA%\dacode\` on
Windows, or wherever `DACODE_CONFIG_DIR` points. See `cli.md` for the full resolution rules.

## Load order

Four layers, each overriding the one before:

1. the built-in defaults in `config::Settings::default`
2. `imported-settings.toml` — written by `dacode import`, never hand-edited
3. `config.toml` — the hand-written TOML, for anything the importer's format can carry
4. `init.lua` — the real config, and therefore the last word

The settings tab (`ctrl-,`) writes into a marked block appended to the end of `init.lua`. Because
Lua applies in order, the block is authoritative over your own `dacode.set` calls above it; deleting
a line from that block, or resetting the setting on the page, hands the setting back to your own code
and to DaCode's default.

**The block holds only what you changed on the page.** A setting is written when you change it
there, and not because it happens to have a value: zooming with `ctrl-+`, switching the theme with a
key, or importing from VSCode changes the live editor without putting anything in your file. Lines
already in the block keep their text. A reset takes the setting's line out rather than writing
today's default, so a later import or a change to the default still reaches you.

**A hand edit wins.** If you change a setting in `init.lua` while the page's own change to the same
setting is still waiting to be written, the file's value stands and the page's is dropped. The
most recent intent is the one kept, and the file is the one you can read. A change to a *different*
setting is still written. See [the configuration guide](../guides/configuration.md) for how saving
works.

A missing file at any layer is not an error, and a broken one is not fatal. Everything an `init.lua`
managed to set before it failed still applies, and the failure is shown in a box at startup rather
than logged.

## `dacode.set` options

| Name | Type | Default | What it does |
|---|---|---|---|
| `vim_keys` | boolean | `true` | Vim's modal grammar. Off means letters always insert and the shortcuts are the conventional ones. |
| `font_size` | number | `13.5` | Size of the code face in pixels. Clamped to 11.0–18.0. |
| `tab_width` | integer | `4` | Columns a tab stands for. Clamped to 1–16. |
| `soft_wrap` | boolean | `true` | Wrap long lines rather than scrolling sideways. |
| `format_on_save` | boolean | `false` | Reformat after every save, through the language server or an external formatter. |
| `theme` | string | `"dacode"` | A file in `themes/`, or one of the thirteen built-ins. File first. |
| `appearance` | `"dark"` \| `"light"` | unset | Flips the built-in theme. Ignored entirely when `theme` is set. |
| `line_numbers` | `"absolute"` \| `"relative"` \| `"off"` | follows `vim_keys` | What the gutter counts. |
| `sticky_scroll` | boolean | `true` | Pin enclosing block heads above the viewport. |
| `inlay_hints` | boolean | `false` | Ask the language server for inferred types and parameter names. |
| `autosave` | boolean | `false` | Write the file a moment after typing stops. |
| `transparency` | number, 0–90 | `0` | How much of the desktop shows through the window, in percent. Backgrounds go see-through; text, highlights and popups stay solid. Needs a compositor, and turning it on from `0` takes effect at the next start. |
| `animations` | `"full"` \| `"reduced"` \| `"off"` | `"full"` | How much the interface moves. See below. |
| `leader` | string | `"<space>"` | What `<leader>` expands to in `dacode.map`. |

Anything not named in a `dacode.set` call is left alone, and unset is not the same as false: reading a
boolean naively would have made every `dacode.set` that omitted `soft_wrap` turn wrapping off, because
Lua coerces a missing key to `false`. So a boolean is taken **only** when the value really is a
boolean — which also means `soft_wrap = "yes"` reads as absent and is not reported. Numbers and
strings go through mlua's own coercion, so `tab_width = "2"` works and `theme = 5` becomes `"5"`.
Bad *values* for `appearance`, `line_numbers` and `animations` are reported by name.

### Notes on the ones that are not obvious

**`animations` has three settings, and `reduced` is not a slower `full`.** `full` is springs with a
little overshoot — about 300 ms for a panel or a popup — and everything that opens fades in and grows
from 0.96 of its size, the sidebar slides, the active tab and the activity marker glide, the bell
swings, and a wheel notch or a page key glides to where it is going. `reduced` keeps every fade and
takes away everything that travels or overshoots: nothing slides, scales, lifts or wobbles, and
a glide arrives without passing its mark. For someone who finds motion uncomfortable, that is the
setting that keeps the information a fade carries. `off` makes every change jump, and switching to it
finishes whatever was in flight at once. Switching *to* `reduced` does not re-base springs already
running, which finish as they were. Typing, the cursor, selections and scrolling that follows the
cursor never animate, whatever this says. Any other spelling is reported by name rather than read as
`full`, and so is `animations = false`, which is the obvious way to write "off" and is not one.

**`line_numbers` has no fixed default.** It follows the grammar: `relative` with `vim_keys` on,
`absolute` with it off. Relative numbering exists so `12j` can be read off the screen, and with the
grammar off there is no count to type — a column of numbers that are not the line's number is then
just noise. `config::LineNumbers::default_for` is the one place that decides, and it runs *after*
`vim_keys` has settled, which is why it is not a plain fallback. A value that is none of the three
is reported as a problem and the default applies.

**`font_size` is clamped twice, at the same limits.** `dacode.set` stores whatever number it is given;
`Typography::with_code_size` then clamps it to 11.0–18.0. Values outside that range are silently
brought inside it, so `font_size = 40` renders at 18. The settings tab takes its bounds from
`Typography` rather than repeating them, so clicking the stepper up to 18 and past it wraps back round
to 11.

**`tab_width` is clamped twice, at the same limits.** `dacode.set` clamps to 1–16 and `config::apply_lua`
clamps again. Never zero, which would divide by it.

**`theme` prefers a file over a built-in.** `config::resolve_theme` tries `themes/<slug>.toml` first
and falls back to `Theme::by_name`. A `themes/dark.toml` you wrote therefore shadows the built-in
called `dark` — the more specific answer wins, and silently preferring the compiled-in one would
leave you editing a file that never loads. The name is slugified, so `theme = "One Dark Pro"` finds
`one-dark-pro.toml`. The built-ins are, dark: `dacode`, `dark`, `rose-pine`, `rose-pine-moon`,
`amoled`, `quizzy-dark`, `coral-dark`, `sky-dark`, `amoled-black`; light: `light`, `quizzy-light`,
`coral-light`, `sky-light`. A theme file with no `appearance` line is dark or light by its `bg`.
A name that matches neither a file nor a built-in is reported with both the list and the path that
was tried.

**`appearance` is a fallback for `theme`, not a companion to it.** `config::apply_lua` consults it
only in the `else` branch of "was a theme named?". Setting both means `appearance` does nothing.

**`format_on_save` and `autosave` are off for the same reason.** Writing or reflowing someone's file
without being asked is not a decision to make on their behalf, and it is the kind of default that
loses trust the first time it fires.

**The theme row opens a list.** A click on its control, or enter or space on the row, opens a card
under the control listing the built-ins under **Dark** and **Light** by their display names (*Quizzy*, *Charcoal coral*,
*Rosé Pine* — both halves of a pair share one, and the heading says which); ↑ ↓ move, enter or a click picks
(applied live and written like any other change), esc or a click outside closes. ← → on the row step
through the same order without opening it.

**A theme loaded from a file cannot be changed by the page.** The page offers the thirteen built-ins by
name; a file's colours have no name it could write, so the row shows `from a file` and writes nothing
for it — otherwise changing the font size would throw away an imported theme.

## `dacode.theme` — individual colours

```lua
dacode.theme {
  bg      = "#101418",
  comment = "oklch(0.62 0.04 250)",
}
```

Both notations are accepted: `#rrggbb` and `oklch(L C H)`. OKLCH is the one worth editing by hand,
because it lets lightness change without shifting the hue. Overrides are applied *after* the theme is
resolved, so they land on top of whichever base was chosen.

Tokens are validated when the config is read, not when the theme is painted, so an unknown name is
reported with the spelling you typed and a suggestion rather than vanishing. The nineteen tokens
are readable from Lua as `dacode.tokens`:

`bg`, `bg_alt`, `bg_panel`, `row_active`, `rule`, `rule_soft`, `ink`, `ink_dim`, `ink_faint`,
`scrim`, `shadow`, `keyword`, `type`, `function`, `attribute`, `comment`, `string`, `number`,
`constant`.

## `dacode.map(mode, keys, action)`

| Argument | Type | Notes |
|---|---|---|
| `mode` | string | `normal`, `insert`, `visual`, `replace`. Single-letter forms `n`, `i`, `v`, `r` also work. |
| `keys` | string | A key *sequence*, not a single key. `<leader>` expands to whatever `leader` was when the binding was made. |
| `action` | string | One of `dacode.actions`. |

Later bindings replace earlier ones for the same mode and sequence, so setting a key twice behaves
like assignment rather than accumulating a binding that can never fire. An empty sequence, an unknown
mode and an unknown action are each reported, the last with a did-you-mean.

`leader` affects bindings added *afterwards*, so a config that changes it must do so before the
`dacode.map` calls that use it.

Bindings are consulted before the built-in grammar, but only when no command is half-typed:
`d<leader>` is the grammar's business, so a config file can never leave an operator dangling. In
insert mode only modified and named keys can be bound — a letter binding would have to hold the
letter back to see what came next, and typing would stutter.

`dacode.actions` is a Lua array of every name `dacode.map` accepts, filtered through `Action::from_name`
so a name that no longer resolves disappears from the list instead of being advertised and then
rejected. There are 79 of them.

## `dacode.server(language, spec)`

```lua
dacode.server("python", { command = ".venv/bin/pylsp" })
dacode.server("zig", { command = "zls", args = { "--stdio" }, extensions = { "zig" } })
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `command` | string | yes | The program that speaks LSP. Empty or missing is reported. |
| `args` | list of strings | no | Passed verbatim. |
| `extensions` | list of strings | conditionally | Without the dot; a leading dot is stripped and the value lowercased. |

The language is positional because it is what the entry is *about*. It is matched against DaCode's own
table — `rust`, `javascript`, `typescript`, `python`, `cpp`, `go`, `lua`, `dart` — case-insensitively.
For one of those, `extensions` may be omitted and DaCode fills in the list it already knows. For any
other name, `extensions` is required, because otherwise nothing would ever reach the server; the error
message says so and shows the shape.

A configured entry goes **ahead** of the built-ins rather than replacing them, so naming a venv's
server does not also throw away the fallback.

**A command containing a path separator is a path, not a name.** `PATH` is a list of directories, so
looking `.venv/bin/pylsp` up in it finds nothing — and a venv's own server is the main reason to name
one. Relative paths resolve against the **project root**, not the process's working directory, which
for a windowed editor is wherever the desktop launched it from. `~/` is expanded. On Windows the
lookup honours `PATHEXT`, so a `.cmd` shim is found.

## `dacode.formatter(language, spec)`

```lua
dacode.formatter("python", { command = ".venv/bin/black", args = { "-" } })
dacode.formatter("nix", { command = "nixfmt", extensions = { "nix" } })
```

The same three fields and the same rules as `dacode.server`, against a different language table:
`rust`, `typescript`, `python`, `cpp`, `go`, `json`, `css`, `markdown`.

A formatter is given the file's text on **stdin** and never a path — one handed the file would
rewrite it behind a dirty buffer. Some programs still need to be told what they are looking at, which
is what `args` is for: `prettier` takes `--stdin-filepath`, `clang-format` takes `--assume-filename`,
and DaCode appends the path to either one.

Whether `Action::Format` goes to the language server or to a formatter is decided at the LSP
handshake, from `documentFormattingProvider` in the `initialize` reply. pyright says no, so a Python
file formats through ruff or black; a plain JavaScript project has no server at all and never gets
as far as asking.

## The TOML layer

`imported-settings.toml` and `config.toml` are read by a hand-rolled parser over `[section]` headers
and `key = value` lines. This layer exists because it is what the VSCode importer *writes* — an
imported theme has to work without anyone having hand-authored a Lua file.

| Key | Type | Maps to |
|---|---|---|
| `editor.font_size` | number | `font_size` |
| `editor.tab_width` | integer | `tab_width` |
| `editor.soft_wrap` | boolean | `soft_wrap` |
| `editor.format_on_save` | boolean | `format_on_save` |
| `theme.name` | string | `theme` |
| `theme.appearance` | `"light"` | flips to the light built-in, unless `theme.name` already resolved |

Six keys, against fourteen in Lua. `vim_keys`, `line_numbers`, `sticky_scroll`, `inlay_hints`,
`autosave`, `transparency`, `animations` and `leader` have no TOML spelling and can only be set from
`init.lua` or the settings tab.

A leading byte-order mark is stripped from both formats. Notepad and PowerShell's `Set-Content` both
write one, and without stripping it Lua refuses the whole file over an invisible byte while the TOML
parser silently loses everything above its second heading.

## Where the three disagree

Adding an option is meant to touch the Lua reader, the settings struct and the settings page. These
do not:

| Option | Lua reader | Settings struct | Settings page |
|---|---|---|---|
| `appearance` | yes | yes | **no row** |
| `leader` | yes | yes (into the keymap) | **no row** |

Both are arguably deliberate — the page has no key capturer, and `appearance` is subsumed by the
theme row it would sit next to — but neither says so. The Keys page lists every command and its
shortcut and can search them, but it cannot rebind one; it has a row that opens `init.lua`, which is
where `leader` and `dacode.map` live.

The page's font-size bounds used to be a third disagreement, and they are worth recording because of
how long it lasted. `settings_ui` clamped to 8.0–32.0 and wrapped a click at 32, while the value is
applied through `Typography::with_code_size`, which clamps to 11.0–18.0, and read back from the live
editor. The page could therefore never reach 32, the wrap never fired, and a mouse-only user who
clicked the font size up to 18 had no way to lower it again. Three unit tests asserted the 8–32
bounds and passed throughout, because they drove a bare `Values` struct rather than the path the
editor uses. Both bounds now come from `Typography` itself, and the test that replaced them asserts
that every size a click can produce is one the typography will keep.

**The importer writes four keys nothing reads.** `ImportedSettings::to_config` emits
`editor.font_family`, `editor.insert_spaces`, `editor.ruler_column` and
`editor.relative_line_numbers`; `config::load_from` reads none of them. They are counted in the
import report, so `dacode import --settings` can claim to have brought over settings that do nothing.

## Example file

`examples/init.lua` is meant to document every option, and `dacode-lua` now loads it through the real
reader in a test — so a renamed option or a mistyped action name fails the build rather than the
first person to follow the instructions.

Three drifts found when that test was written have been corrected: it claimed a built-in theme wins
over a file of the same name (the opposite is true — `resolve_theme` tries the file first, and there
is a test named for it), it set `line_numbers = "absolute"` under a header claiming the values shown
are the defaults when with `vim_keys = true` the default is `relative`, and it never mentioned
`appearance`, `dacode.server` or `dacode.formatter` at all.

One remains: it shows only the long mode names for `dacode.map`, where `n`, `i`, `v` and `r` also work.

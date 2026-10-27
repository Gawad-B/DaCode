-- dacode configuration.
--
-- Copy to ~/.config/dacode/init.lua (or $DACODE_CONFIG_DIR/init.lua) and edit. Every line is optional:
-- dacode starts with no configuration at all, and the defaults here are the built-in ones.
--
-- Mistakes are reported, not swallowed. A typo in an action name, an unknown theme token or a
-- syntax error shows up in a box when the editor opens, and everything the file managed to set
-- before the error still applies.

dacode.set {
  -- Vim's modal editing. Turn it off and dacode behaves like any other editor: typing always inserts,
  -- and the shortcuts are the familiar ones — ctrl-c, ctrl-v, ctrl-z, shift-arrow to select. The
  -- editor's own chords (ctrl-e, ctrl-p, ctrl-w, ctrl-,) keep working either way.
  vim_keys = true,

  font_size = 13.5,
  tab_width = 4,
  soft_wrap = true,

  -- Reformat through the language server after every save. Off by default: a formatter reflowing a
  -- file you did not ask it to is worse than no formatter. The save itself always lands first — the
  -- reformat is a round trip, and it writes again when it comes back.
  format_on_save = false,

  -- One of the twelve built-in themes. Dark: slate, walnut, rose-pine, rose-pine-moon, true-black,
  -- espresso, charcoal-coral, abyss. Light: linen, cream, mist-coral, snow.
  -- Or the name of a file in ~/.config/dacode/themes/ — which is what `dacode import --theme` writes,
  -- slugified, so "One Dark Pro" finds one-dark-pro.toml.
  --
  -- The *file* is tried first, so putting walnut.toml in themes/ replaces the built-in walnut rather
  -- than being ignored by it. That is what makes a built-in name editable at all.
  theme = "slate",

  -- Light or dark, where a theme has not settled it. "dark" or "light".
  -- appearance = "dark",

  -- Pin the enclosing block heads above the viewport while scrolling.
  sticky_scroll = false,

  -- Ask the language server for inferred types and parameter names, shown inline.
  inlay_hints = false,

  -- What the gutter counts: "absolute", "relative" (vim's, so `12j` can be read off the screen),
  -- or "off". Unset, it follows vim_keys — relative with the grammar on, plain numbers without it,
  -- so this line is the one setting here that is *not* the default: with vim_keys above, the
  -- default is "relative".
  line_numbers = "absolute",

  -- Write the file a moment after you stop typing, rather than waiting for ctrl-s. It waits for a
  -- pause rather than running on a timer, so what lands on disk is a state you stopped at.
  autosave = false,

  -- How much of the desktop shows through the window, in percent, from 0 to 90. Backgrounds go
  -- see-through; text and popups stay solid. Needs a compositor, and turning it on from 0 takes
  -- effect at the next start.
  transparency = 0,

  -- How much the interface moves: "full" (springs that settle with a little overshoot), "reduced"
  -- (short fades, nothing that travels) or "off". A string, so `animations = false` is an error
  -- rather than a quiet "full".
  animations = "full",

  -- What <leader> means in the bindings below.
  leader = "<space>",
}

-- Individual colors, over whatever the theme set. Either notation works: oklch(L C H) is the one
-- worth editing by hand, because it lets you change lightness without shifting the hue.
dacode.theme {
  -- bg      = "#101418",
  -- comment = "oklch(0.62 0.04 250)",
}

-- A language server, driven over LSP. Yours is tried *first* and dacode's own list stays behind it, so
-- naming a virtualenv does not cost you the fallback on a machine where that virtualenv is absent.
--
-- A command containing a separator is a path rather than a name to look for on PATH, and a relative
-- one resolves against the **project root** — not the directory the editor was launched from, which
-- for a desktop icon is nobody's project. `~/` means your home directory.
--
-- dacode.server("python", { command = ".venv/bin/pylsp" })
--
-- A language dacode has never heard of has to say which files it serves.
-- dacode.server("zig", { command = "zls", extensions = { "zig", "zon" } })

-- An external formatter, for when the language server does not format — pyright does not, and a
-- plain JavaScript or Markdown project has no server at all. Same shape, because it is the same
-- kind of fact. Text goes in on stdin and comes back on stdout; the formatter is never handed the
-- path, because one given the file would rewrite it behind a buffer you have unsaved changes in.
--
-- dacode.formatter("python", { command = ".venv/bin/black", args = { "-" } })
-- dacode.formatter("nix", { command = "nixfmt", extensions = { "nix" } })

-- Bindings. The third argument is one of `dacode.actions` — print it to see the whole list.
--
-- These are consulted before the built-in Vim grammar, but only when no command is half-typed:
-- `d<leader>` is the grammar's business, not a binding's, so an operator can never be left
-- dangling by a config file.
dacode.map("normal", "<leader>w", "save")
dacode.map("normal", "<leader>q", "quit")
dacode.map("normal", "<leader>f", "search_project")
dacode.map("normal", "<leader>d", "goto_definition")
dacode.map("normal", "<leader>k", "hover")
dacode.map("normal", "<leader>z", "toggle_soft_wrap")
dacode.map("normal", "<leader>n", "rename")
dacode.map("normal", "<leader>=", "format")

-- In insert mode only modified and named keys can be bound: a letter binding would have to hold
-- the letter back to see what came next, and typing would stutter.
dacode.map("insert", "<c-s>", "save")

-- Config is a program, which is the reason it is Lua rather than a data format.
for i, action in ipairs { "keep_primary_selection", "split_selection_into_lines" } do
  dacode.map("visual", tostring(i), action)
end

-- vex configuration.
--
-- Copy to ~/.config/vex/init.lua (or $VEX_CONFIG_DIR/init.lua) and edit. Every line is optional:
-- vex starts with no configuration at all, and the defaults here are the built-in ones.
--
-- Mistakes are reported, not swallowed. A typo in an action name, an unknown theme token or a
-- syntax error shows up in a box when the editor opens, and everything the file managed to set
-- before the error still applies.

vex.set {
  -- Vim's modal editing. Turn it off and vex behaves like any other editor: typing always inserts,
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

  -- One of the five built-in themes: dark, light, rose-pine, rose-pine-moon, amoled.
  -- Or the name of a file in ~/.config/vex/themes/ — which is what `vex import --theme` writes,
  -- slugified, so "One Dark Pro" finds one-dark-pro.toml.
  --
  -- The *file* is tried first, so putting dark.toml in themes/ replaces the built-in dark rather
  -- than being ignored by it. That is what makes a built-in name editable at all.
  theme = "dark",

  -- Light or dark, where a theme has not settled it. "dark" or "light".
  -- appearance = "dark",

  -- Pin the enclosing block heads above the viewport while scrolling.
  sticky_scroll = true,

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

  -- What <leader> means in the bindings below.
  leader = "<space>",
}

-- Individual colors, over whatever the theme set. Either notation works: oklch(L C H) is the one
-- worth editing by hand, because it lets you change lightness without shifting the hue.
vex.theme {
  -- bg      = "#101418",
  -- comment = "oklch(0.62 0.04 250)",
}

-- A language server, driven over LSP. Yours is tried *first* and vex's own list stays behind it, so
-- naming a virtualenv does not cost you the fallback on a machine where that virtualenv is absent.
--
-- A command containing a separator is a path rather than a name to look for on PATH, and a relative
-- one resolves against the **project root** — not the directory the editor was launched from, which
-- for a desktop icon is nobody's project. `~/` means your home directory.
--
-- vex.server("python", { command = ".venv/bin/pylsp" })
--
-- A language vex has never heard of has to say which files it serves.
-- vex.server("zig", { command = "zls", extensions = { "zig", "zon" } })

-- An external formatter, for when the language server does not format — pyright does not, and a
-- plain JavaScript or Markdown project has no server at all. Same shape, because it is the same
-- kind of fact. Text goes in on stdin and comes back on stdout; the formatter is never handed the
-- path, because one given the file would rewrite it behind a buffer you have unsaved changes in.
--
-- vex.formatter("python", { command = ".venv/bin/black", args = { "-" } })
-- vex.formatter("nix", { command = "nixfmt", extensions = { "nix" } })

-- Bindings. The third argument is one of `vex.actions` — print it to see the whole list.
--
-- These are consulted before the built-in Vim grammar, but only when no command is half-typed:
-- `d<leader>` is the grammar's business, not a binding's, so an operator can never be left
-- dangling by a config file.
vex.map("normal", "<leader>w", "save")
vex.map("normal", "<leader>q", "quit")
vex.map("normal", "<leader>f", "search_project")
vex.map("normal", "<leader>d", "goto_definition")
vex.map("normal", "<leader>k", "hover")
vex.map("normal", "<leader>z", "toggle_soft_wrap")
vex.map("normal", "<leader>n", "rename")
vex.map("normal", "<leader>=", "format")

-- In insert mode only modified and named keys can be bound: a letter binding would have to hold
-- the letter back to see what came next, and typing would stutter.
vex.map("insert", "<c-s>", "save")

-- Config is a program, which is the reason it is Lua rather than a data format.
for i, action in ipairs { "keep_primary_selection", "split_selection_into_lines" } do
  vex.map("visual", tostring(i), action)
end

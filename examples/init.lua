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
  -- slugified, so "One Dark Pro" finds one-dark-pro.toml. A built-in of the same name wins.
  theme = "dark",

  -- Pin the enclosing block heads above the viewport while scrolling.
  sticky_scroll = true,

  -- Ask the language server for inferred types and parameter names, shown inline.
  inlay_hints = false,

  -- What <leader> means in the bindings below.
  leader = "<space>",
}

-- Individual colors, over whatever the theme set. Either notation works: oklch(L C H) is the one
-- worth editing by hand, because it lets you change lightness without shifting the hue.
vex.theme {
  -- bg      = "#101418",
  -- comment = "oklch(0.62 0.04 250)",
}

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

//! `vex-lua` — configuration as Lua.
//!
//! Config here is *code* rather than a data format, which was a decision made before the first line
//! of the editor was written. The reason is keybindings: a binding maps a key sequence to the same
//! [`Action`](vex_input::Action) vocabulary the grammar emits, and the useful ones are conditional
//! ("this only when a language server is attached") or generated in a loop. A static table can hold
//! `font_size = 14`; it cannot hold that.
//!
//! **What this module deliberately does not do:** it does not stay alive. The interpreter runs once
//! at startup, produces a plain [`Config`] value, and is dropped before the first frame. There is no
//! Lua on the UI thread, no callback into Lua per keystroke, and therefore no way for a slow config
//! to violate the 60fps contract. When Lua callbacks eventually become worth having (autocommands,
//! custom commands), they get their own budget and their own thread — the runtime does not get
//! promoted into the render loop by accident.
//!
//! **Errors never stop the editor.** A config file with a syntax error, a bad action name, or a
//! runtime failure halfway through leaves everything it had already set in place, records a
//! diagnostic, and startup continues with defaults for the rest. An editor that refuses to open
//! because of a typo in its own config is an editor you cannot use to fix the typo.

use mlua::{Lua, Table, Value};
use std::path::Path;
use vex_input::Keymap;
use vex_theme::{Appearance, Oklch, Theme};

/// Everything an `init.lua` can set.
///
/// Options are `Option` so an unset value means "keep the default" rather than "reset to zero" —
/// which also lets a Lua file and the TOML importer layer without either having to know about the
/// other.
#[derive(Debug, Default, Clone)]
pub struct Config {
    pub font_size: Option<f32>,
    pub tab_width: Option<usize>,
    pub soft_wrap: Option<bool>,
    pub format_on_save: Option<bool>,
    pub appearance: Option<Appearance>,
    /// A named theme file to load, resolved by the caller — this crate does no file I/O beyond
    /// reading the config itself.
    pub theme_name: Option<String>,
    /// Token overrides from `vex.theme{...}`, in the order they were written.
    pub colors: Vec<(String, Oklch)>,
    pub keymap: Keymap,
    /// Everything that went wrong, in the user's terms. Shown rather than swallowed.
    pub problems: Vec<String>,
}

impl Config {
    /// Applies the `vex.theme{...}` overrides to a theme.
    pub fn paint(&self, theme: &mut Theme) {
        for (token, color) in &self.colors {
            theme.set_token(token, *color);
        }
    }

    pub fn is_default(&self) -> bool {
        self.font_size.is_none()
            && self.tab_width.is_none()
            && self.soft_wrap.is_none()
            && self.format_on_save.is_none()
            && self.appearance.is_none()
            && self.theme_name.is_none()
            && self.colors.is_empty()
            && self.keymap.is_empty()
    }
}

/// Runs a config file, if it exists. A missing file is not a problem: the built-in defaults are the
/// product's real defaults, and the editor must start with no configuration at all.
pub fn load(path: &Path) -> Config {
    let Ok(source) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    let name = path.display().to_string();
    eval(&source, &name)
}

/// Evaluates config source. Separated from [`load`] so the API is testable without touching a disk.
pub fn eval(source: &str, name: &str) -> Config {
    // `Lua::new()` is the sandboxed-by-omission choice: the standard library is present (a config
    // that cannot use `string.format` or `pairs` is a worse language than TOML), but nothing of
    // vex's is exposed beyond the `vex` table built below.
    let lua = Lua::new();
    let state = std::rc::Rc::new(std::cell::RefCell::new(Config::default()));

    if let Err(error) = install(&lua, &state) {
        let mut config = Config::default();
        config
            .problems
            .push(format!("could not create the config API: {error}"));
        return config;
    }

    if let Err(error) = lua.load(source).set_name(name).exec() {
        // Whatever ran before the error stands. A file that sets the theme and then fails on line 40
        // should still be themed.
        state.borrow_mut().problems.push(describe(&error));
    }

    drop(lua);
    std::rc::Rc::try_unwrap(state)
        .map(|cell| cell.into_inner())
        .unwrap_or_default()
}

type Shared = std::rc::Rc<std::cell::RefCell<Config>>;

fn install(lua: &Lua, state: &Shared) -> mlua::Result<()> {
    let vex = lua.create_table()?;

    // `vex.set{...}` — plain options.
    let target = state.clone();
    vex.set(
        "set",
        lua.create_function(move |_, options: Table| {
            let mut config = target.borrow_mut();

            if let Some(size) = number(&options, "font_size") {
                config.font_size = Some(size as f32);
            }
            if let Some(width) = number(&options, "tab_width") {
                config.tab_width = Some((width as usize).clamp(1, 16));
            }
            if let Some(wrap) = boolean(&options, "soft_wrap") {
                config.soft_wrap = Some(wrap);
            }
            if let Some(format) = boolean(&options, "format_on_save") {
                config.format_on_save = Some(format);
            }
            if let Some(appearance) = string(&options, "appearance") {
                match appearance.as_str() {
                    "dark" => config.appearance = Some(Appearance::Dark),
                    "light" => config.appearance = Some(Appearance::Light),
                    other => config.problems.push(format!(
                        "appearance must be \"dark\" or \"light\", got {other:?}"
                    )),
                }
            }
            if let Some(name) = string(&options, "theme") {
                config.theme_name = Some(name);
            }
            if let Some(leader) = string(&options, "leader") {
                config.keymap.set_leader(&leader);
            }
            Ok(())
        })?,
    )?;

    // `vex.theme{...}` — token overrides.
    let target = state.clone();
    vex.set(
        "theme",
        lua.create_function(move |_, colors: Table| {
            let mut config = target.borrow_mut();
            for pair in colors.pairs::<String, Value>() {
                let (token, value) = pair?;
                let Value::String(raw) = value else {
                    config
                        .problems
                        .push(format!("theme.{token} must be a color string"));
                    continue;
                };
                let raw = raw.to_string_lossy();

                match Oklch::parse(&raw) {
                    Some(color) => {
                        // Validated here rather than at paint time, so an unknown token is reported
                        // with the name the user typed instead of vanishing.
                        if Theme::dark().set_token(&token, color) {
                            config.colors.push((token, color));
                        } else {
                            config.problems.push(match nearest(&token, &Theme::TOKENS) {
                                Some(guess) => format!(
                                    "unknown theme token `{token}` — did you mean `{guess}`?"
                                ),
                                // No close match: listing all nineteen would not fit the box that
                                // shows this, and `vex.tokens` is the list.
                                None => format!(
                                    "unknown theme token `{token}` (see vex.tokens for the list)"
                                ),
                            });
                        }
                    }
                    None => config.problems.push(format!(
                        "could not read color {raw:?} for `{token}` — expected \"#rrggbb\" or \"oklch(L C H)\""
                    )),
                }
            }
            Ok(())
        })?,
    )?;

    // `vex.map(mode, keys, action)` — bindings.
    let target = state.clone();
    vex.set(
        "map",
        lua.create_function(move |_, (mode, keys, action): (String, String, String)| {
            let mut config = target.borrow_mut();
            if let Err(error) = config.keymap.bind(&mode, &keys, &action) {
                let hint = match nearest(&action, &action_names()) {
                    Some(guess) if guess != action => format!(" — did you mean `{guess}`?"),
                    _ => String::new(),
                };
                config
                    .problems
                    .push(format!("vex.map(\"{mode}\", \"{keys}\", …): {error}{hint}"));
            }
            Ok(())
        })?,
    )?;

    // The action vocabulary, readable from Lua. Config that can enumerate what it may bind to is
    // config a user can explore without leaving their editor.
    let actions = lua.create_table()?;
    for (index, name) in action_names().iter().enumerate() {
        actions.set(index + 1, *name)?;
    }
    vex.set("actions", actions)?;
    vex.set("tokens", Theme::TOKENS.to_vec())?;

    lua.globals().set("vex", vex)?;
    Ok(())
}

/// Every name `vex.map` accepts. Derived by asking [`vex_input::Action`] rather than kept as a
/// second list, so the two cannot disagree.
pub fn action_names() -> Vec<&'static str> {
    CANDIDATE_ACTIONS
        .iter()
        .copied()
        .filter(|name| vex_input::Action::from_name(name).is_some())
        .collect()
}

/// The names this crate advertises. Filtered through `Action::from_name`, so a name deleted from the
/// vocabulary disappears from `vex.actions` instead of being advertised and then rejected.
const CANDIDATE_ACTIONS: [&str; 60] = [
    "move_left",
    "move_right",
    "move_up",
    "move_down",
    "move_word_forward",
    "move_word_backward",
    "move_word_end",
    "move_line_start",
    "move_line_end",
    "move_first_non_blank",
    "move_file_start",
    "move_file_end",
    "move_matching_bracket",
    "move_paragraph_forward",
    "move_paragraph_backward",
    "normal_mode",
    "insert_mode",
    "visual_mode",
    "replace_mode",
    "insert_before",
    "insert_after",
    "insert_line_start",
    "insert_line_end",
    "open_below",
    "open_above",
    "newline",
    "delete_backward",
    "delete_forward",
    "paste",
    "paste_before",
    "undo",
    "redo",
    "repeat_change",
    "add_caret_above",
    "add_caret_below",
    "split_selection_into_lines",
    "keep_primary_selection",
    "collapse_selections",
    "select_line",
    "select_all",
    "toggle_soft_wrap",
    "toggle_theme",
    "increase_font_size",
    "decrease_font_size",
    "save",
    "quit",
    "search",
    "search_backward",
    "search_next",
    "search_previous",
    "command",
    "search_project",
    "hover",
    "goto_definition",
    "next_diagnostic",
    "previous_diagnostic",
    "dismiss_info",
    "completion",
    "rename",
    "format",
];

/// The closest name in `candidates`, if one is close enough to be worth suggesting.
///
/// A suggestion beats a catalogue: a typo is nearly always one or two characters, and the message
/// has to fit in a box on screen.
fn nearest<'a>(typo: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let limit = match typo.chars().count() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    candidates
        .iter()
        .map(|name| (distance(typo, name), *name))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, name)| (*d, name.len()))
        .map(|(_, name)| name)
}

/// Levenshtein distance, two rows at a time. Over strings this short the algorithm choice does not
/// matter; it is here so a typo can be named rather than answered with a list.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];

    for (i, ac) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ac != *bc);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

fn number(table: &Table, key: &str) -> Option<f64> {
    table.get::<f64>(key).ok()
}

/// Reads a boolean **only if the key is present**.
///
/// Not `get::<bool>`: Lua's truthiness means mlua converts a missing key to `false` rather than
/// failing, so every `vex.set{...}` that did not mention `soft_wrap` was turning soft wrap off.
fn boolean(table: &Table, key: &str) -> Option<bool> {
    match table.get::<Value>(key).ok()? {
        Value::Boolean(value) => Some(value),
        _ => None,
    }
}

fn string(table: &Table, key: &str) -> Option<String> {
    table.get::<String>(key).ok()
}

/// Turns a Lua error into one line a user can act on.
///
/// mlua's own text carries a traceback and a `runtime error:` prefix that says nothing to someone
/// who wrote three lines of config; the file and line number are the parts that matter.
fn describe(error: &mlua::Error) -> String {
    let text = match error {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(message) => message.clone(),
        mlua::Error::CallbackError { cause, .. } => return describe(cause),
        other => other.to_string(),
    };
    text.lines()
        .next()
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vex_input::{Action, Key, Match};
    use vex_theme::Mode;

    fn run(source: &str) -> Config {
        eval(source, "test.lua")
    }

    #[test]
    fn options_are_read() {
        let config = run(r##"
            vex.set {
              font_size = 15.5,
              tab_width = 2,
              soft_wrap = false,
              appearance = "light",
              theme = "one-dark",
            }
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.font_size, Some(15.5));
        assert_eq!(config.tab_width, Some(2));
        assert_eq!(config.soft_wrap, Some(false));
        assert_eq!(config.appearance, Some(Appearance::Light));
        assert_eq!(config.theme_name.as_deref(), Some("one-dark"));
    }

    #[test]
    fn bindings_reach_the_keymap() {
        let config = run(r##"
            vex.set { leader = "," }
            vex.map("normal", "<leader>w", "save")
            vex.map("normal", "<leader>f", "search_project")
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(
            config
                .keymap
                .lookup(Mode::Normal, &Key::parse_sequence(",w")),
            Match::Exact(Action::Save)
        );
    }

    #[test]
    fn theme_tokens_accept_both_notations() {
        let config = run(r##"
            vex.theme {
              bg = "#101418",
              keyword = "oklch(0.7 0.16 318)",
            }
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        let mut theme = Theme::dark();
        config.paint(&mut theme);
        assert_eq!(theme.bg.to_hex(), "#101418");
        assert!((theme.syntax.keyword.h - 318.0).abs() < 0.5);
    }

    #[test]
    fn config_is_a_program_not_a_table() {
        // The reason this is Lua at all: bindings can be generated. If this stops working, the
        // dependency is not paying for itself.
        let config = run(r##"
            for _, pair in ipairs { { "j", "move_down" }, { "k", "move_up" } } do
              vex.map("normal", pair[1], pair[2])
            end
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.keymap.len(), 2);
    }

    #[test]
    fn an_error_partway_through_keeps_what_already_ran() {
        let config = run(r##"
            vex.set { tab_width = 8 }
            error("something went wrong")
            vex.set { tab_width = 2 }
        "##);

        assert_eq!(
            config.tab_width,
            Some(8),
            "settings from before the error survive"
        );
        assert_eq!(config.problems.len(), 1);
        assert!(
            config.problems[0].contains("something went wrong"),
            "{:?}",
            config.problems
        );
    }

    #[test]
    fn a_syntax_error_is_reported_with_its_line() {
        let config = run("vex.set { tab_width = }");

        assert_eq!(config.problems.len(), 1);
        assert!(
            config.problems[0].contains("test.lua"),
            "the message names the file: {:?}",
            config.problems
        );
        assert!(config.is_default(), "nothing was applied");
    }

    #[test]
    fn typos_are_named_rather_than_ignored() {
        let config = run(r##"
            vex.map("normal", "<leader>x", "sav")
            vex.theme { bakcground = "#000000" }
            vex.theme { bg = "not a color" }
            vex.set { appearance = "twilight" }
        "##);

        assert_eq!(config.problems.len(), 4, "{:?}", config.problems);
        let all = config.problems.join("\n");
        assert!(all.contains("unknown action `sav`"), "{all}");
        assert!(all.contains("bakcground"), "{all}");
        assert!(all.contains("not a color"), "{all}");
        assert!(all.contains("twilight"), "{all}");
        assert!(
            config.keymap.is_empty() && config.colors.is_empty(),
            "nothing broken was applied"
        );
    }

    #[test]
    fn no_config_at_all_is_valid() {
        let config = run("");
        assert!(config.problems.is_empty());
        assert!(config.is_default());

        let missing = load(Path::new("/nonexistent/vex/init.lua"));
        assert!(
            missing.problems.is_empty(),
            "a missing file is not an error"
        );
        assert!(missing.is_default());
    }

    #[test]
    fn the_vocabulary_is_introspectable_from_lua() {
        // `vex.actions` exists so a user can discover what they may bind without reading the source.
        let config = run(r##"
            local found = false
            for _, name in ipairs(vex.actions) do
              if name == "goto_definition" then found = true end
            end
            if not found then error("goto_definition is not advertised") end
            if #vex.tokens == 0 then error("no theme tokens advertised") end
        "##);
        assert!(config.problems.is_empty(), "{:?}", config.problems);
    }

    #[test]
    fn every_advertised_action_resolves() {
        // The advertised list is filtered through `Action::from_name`, so this asserts the filter is
        // not quietly hiding most of it.
        assert_eq!(
            action_names().len(),
            CANDIDATE_ACTIONS.len(),
            "an advertised action name no longer resolves"
        );
    }

    #[test]
    fn an_unmentioned_option_stays_unset() {
        // Lua has no distinction between false and absent when coerced, so reading a bool naively
        // made every `vex.set` that omitted `soft_wrap` turn wrapping off.
        let config = run("vex.set { tab_width = 2 }");
        assert_eq!(config.soft_wrap, None, "absent is not false");

        let explicit = run("vex.set { soft_wrap = false }");
        assert_eq!(explicit.soft_wrap, Some(false));

        let on = run("vex.set { soft_wrap = true }");
        assert_eq!(on.soft_wrap, Some(true));
    }

    #[test]
    fn a_typo_gets_a_suggestion_rather_than_a_catalogue() {
        // The message is shown in a box a few lines tall, so listing nineteen tokens is not an
        // answer. A one-character typo should be named.
        let config = run(r##"
            vex.theme { bg_pannel = "#101418" }
            vex.map("normal", "x", "sav")
        "##);

        let all = config.problems.join("\n");
        assert!(all.contains("did you mean `bg_panel`"), "{all}");
        assert!(all.contains("did you mean `save`"), "{all}");
        assert!(
            !all.contains("bg_alt, bg_panel"),
            "the whole token list should not be in the message: {all}"
        );
    }

    #[test]
    fn a_name_with_no_close_match_points_at_the_list() {
        // `backgruond` is a typo of `background`, which is not a vex token at all — vex calls it
        // `bg` — so there is nothing near enough to suggest.
        let config = run(r##"vex.theme { backgruond = "#101418" }"##);
        assert_eq!(config.problems.len(), 1);
        assert!(
            config.problems[0].contains("vex.tokens"),
            "{:?}",
            config.problems
        );
    }
}

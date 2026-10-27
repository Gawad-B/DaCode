//! `dacode-lua` — configuration as Lua.
//!
//! Config here is *code* rather than a data format, which was a decision made before the first line
//! of the editor was written. The reason is keybindings: a binding maps a key sequence to the same
//! [`Action`](dacode_input::Action) vocabulary the grammar emits, and the useful ones are conditional
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

use dacode_input::Keymap;
use dacode_theme::{Appearance, Oklch, Theme};
use mlua::{Lua, Table, Value};
use std::path::Path;

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
    /// Vim's modal grammar. Off means letters insert and the shortcuts are the usual ones.
    pub modal: Option<bool>,
    pub sticky_scroll: Option<bool>,
    pub inlay_hints: Option<bool>,
    /// Write the file a moment after typing stops.
    pub autosave: Option<bool>,
    /// How much of the desktop shows through the window, as a percentage: 0 is opaque.
    pub transparency: Option<u8>,
    /// How much the interface moves: "full", "reduced" or "off". Checked against [`ANIMATIONS`] here
    /// and parsed into the enum by the caller, which owns it.
    pub animations: Option<String>,
    /// "absolute", "relative" or "off" — parsed by the caller, which owns the enum.
    pub line_numbers: Option<String>,
    pub appearance: Option<Appearance>,
    /// A named theme file to load, resolved by the caller — this crate does no file I/O beyond
    /// reading the config itself.
    pub theme_name: Option<String>,
    /// Token overrides from `dacode.theme{...}`, in the order they were written.
    pub colors: Vec<(String, Oklch)>,
    pub keymap: Keymap,
    /// Language servers named by `dacode.server(...)`, in the order they were written.
    pub servers: Vec<Server>,
    /// Formatters named by `dacode.formatter(...)`. Described exactly as a server is — a language, a
    /// command, its arguments and the files it applies to — so one struct serves both.
    pub formatters: Vec<Server>,
    /// Everything that went wrong, in the user's terms. Shown rather than swallowed.
    pub problems: Vec<String>,
}

/// The spellings `animations` accepts. This crate cannot see `dacode::motion::Animations`, so the
/// binary has a test that every name that enum prints is in this list.
pub const ANIMATIONS: [&str; 3] = ["full", "reduced", "off"];

/// A language server named by `dacode.server(language, { command = …, args = {…} })`.
///
/// Kept as plain strings, validated by the caller. This crate does not know what languages exist —
/// `dacode-lsp` owns that table, and duplicating it here is how the two would come to disagree about
/// what `"python"` means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    /// File extensions, without the dot. Empty means "DaCode already knows which files this language
    /// is written in".
    pub extensions: Vec<String>,
}

impl Config {
    /// Applies the `dacode.theme{...}` overrides to a theme.
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
            && self.modal.is_none()
            && self.sticky_scroll.is_none()
            && self.inlay_hints.is_none()
            && self.autosave.is_none()
            && self.transparency.is_none()
            && self.animations.is_none()
            && self.line_numbers.is_none()
            && self.appearance.is_none()
            && self.theme_name.is_none()
            && self.colors.is_empty()
            && self.keymap.is_empty()
            // A file whose only content is `dacode.server(...)` is not a default config, and leaving
            // this out would have had the caller skip it entirely — the config would be read,
            // parsed, and then thrown away for looking empty.
            && self.servers.is_empty()
            && self.formatters.is_empty()
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
///
/// A leading byte-order mark is dropped first. Notepad and PowerShell's `Set-Content` both write one
/// by default, so on Windows it is the *likely* state of a file someone edited without thinking about
/// encodings — and Lua sees it as a stray character on line 1 and refuses the whole file. Config that
/// fails because of an invisible byte the editor of choice inserted is the worst kind of error to be
/// handed at startup.
pub fn eval(source: &str, name: &str) -> Config {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);

    // `Lua::new()` is the sandboxed-by-omission choice: the standard library is present (a config
    // that cannot use `string.format` or `pairs` is a worse language than TOML), but nothing of
    // DaCode's is exposed beyond the `dacode` table built below.
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
    let dacode = lua.create_table()?;

    // `dacode.set{...}` — plain options.
    let target = state.clone();
    dacode.set(
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
            if let Some(modal) = boolean(&options, "vim_keys") {
                config.modal = Some(modal);
            }
            if let Some(sticky) = boolean(&options, "sticky_scroll") {
                config.sticky_scroll = Some(sticky);
            }
            if let Some(hints) = boolean(&options, "inlay_hints") {
                config.inlay_hints = Some(hints);
            }
            if let Some(autosave) = boolean(&options, "autosave") {
                config.autosave = Some(autosave);
            }
            // Capped at 90: past that the window is a ghost of itself, and the text on it — which
            // stays solid — floats over whatever is behind with nothing to read it against.
            if let Some(percent) = number(&options, "transparency") {
                config.transparency = Some(percent.clamp(0.0, 90.0).round() as u8);
            }
            // Validated here, unlike `line_numbers`, because a value that is not one of the three has
            // no sensible reading: "none" would otherwise leave the editor animating while its owner
            // believed they had turned motion off.
            //
            // Read as a raw value rather than through `string`, which turns a wrong type into
            // "absent": `animations = false` is the most natural way to write "off", and it would be
            // dropped without a word.
            match options.get::<Value>("animations") {
                Ok(Value::Nil) | Err(_) => {}
                Ok(Value::String(name)) => {
                    let name = name.to_string_lossy();
                    if ANIMATIONS.contains(&name.as_str()) {
                        config.animations = Some(name);
                    } else {
                        config.problems.push(format!(
                            "animations must be \"full\", \"reduced\" or \"off\", got {name:?}"
                        ));
                    }
                }
                Ok(_) => config.problems.push(
                    "animations must be \"full\", \"reduced\" or \"off\" (a string, not a \
                     boolean or number)"
                        .into(),
                ),
            }
            // Kept as a string here: this crate owns no editor types, and the caller that has the
            // enum is also the one that can report a bad value in the user's terms.
            if let Some(numbers) = string(&options, "line_numbers") {
                config.line_numbers = Some(numbers);
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

    // `dacode.theme{...}` — token overrides.
    let target = state.clone();
    dacode.set(
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
                        if Theme::walnut().set_token(&token, color) {
                            config.colors.push((token, color));
                        } else {
                            config.problems.push(match nearest(&token, &Theme::TOKENS) {
                                Some(guess) => format!(
                                    "unknown theme token `{token}` — did you mean `{guess}`?"
                                ),
                                // No close match: listing all nineteen would not fit the box that
                                // shows this, and `dacode.tokens` is the list.
                                None => format!(
                                    "unknown theme token `{token}` (see dacode.tokens for the list)"
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

    // `dacode.map(mode, keys, action)` — bindings.
    let target = state.clone();
    dacode.set(
        "map",
        lua.create_function(move |_, (mode, keys, action): (String, String, String)| {
            let mut config = target.borrow_mut();
            if let Err(error) = config.keymap.bind(&mode, &keys, &action) {
                let hint = match nearest(&action, &action_names()) {
                    Some(guess) if guess != action => format!(" — did you mean `{guess}`?"),
                    _ => String::new(),
                };
                config.problems.push(format!(
                    "dacode.map(\"{mode}\", \"{keys}\", …): {error}{hint}"
                ));
            }
            Ok(())
        })?,
    )?;

    // `dacode.server(language, { command = …, args = {…}, extensions = {…} })` — a language server.
    //
    // The language comes first and positionally because it is what the entry is *about*; everything
    // else is a detail of how to start it. Validated by the caller, which owns the language table.
    let target = state.clone();
    dacode.set(
        "server",
        lua.create_function(move |_, (language, options): (String, Table)| {
            let mut config = target.borrow_mut();
            let command = string(&options, "command").unwrap_or_default();
            config.servers.push(Server {
                language,
                command,
                args: strings(&options, "args"),
                extensions: strings(&options, "extensions"),
            });
            Ok(())
        })?,
    )?;

    // `dacode.formatter(language, { command = …, args = {…}, extensions = {…} })` — an external
    // formatter. The same shape as `dacode.server`, because it is the same kind of fact.
    let target = state.clone();
    dacode.set(
        "formatter",
        lua.create_function(move |_, (language, options): (String, Table)| {
            let mut config = target.borrow_mut();
            let command = string(&options, "command").unwrap_or_default();
            config.formatters.push(Server {
                language,
                command,
                args: strings(&options, "args"),
                extensions: strings(&options, "extensions"),
            });
            Ok(())
        })?,
    )?;

    // The action vocabulary, readable from Lua. Config that can enumerate what it may bind to is
    // config a user can explore without leaving their editor.
    let actions = lua.create_table()?;
    for (index, name) in action_names().iter().enumerate() {
        actions.set(index + 1, *name)?;
    }
    dacode.set("actions", actions)?;
    dacode.set("tokens", Theme::TOKENS.to_vec())?;

    // `vex` is the same table under the editor's old name, so an init.lua written before the rename
    // still runs — the settings panel's own block in it included.
    lua.globals().set("vex", dacode.clone())?;
    lua.globals().set("dacode", dacode)?;
    Ok(())
}

/// Every name `dacode.map` accepts: [`dacode_input::Action::NAMES`], the table the palette, the
/// help panel and config already share.
///
/// This was a hand-kept list of its own, filtered through `Action::from_name` so a deleted name
/// could not be advertised. The filter caught deletions and nothing caught additions: the list sat
/// at 79 names while the vocabulary grew past it, so `dacode.actions` never offered
/// `focus_explorer`, `focus_editor` or anything else added after it was written.
pub fn action_names() -> Vec<&'static str> {
    dacode_input::Action::NAMES.to_vec()
}

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
/// failing, so every `dacode.set{...}` that did not mention `soft_wrap` was turning soft wrap off.
fn boolean(table: &Table, key: &str) -> Option<bool> {
    match table.get::<Value>(key).ok()? {
        Value::Boolean(value) => Some(value),
        _ => None,
    }
}

fn string(table: &Table, key: &str) -> Option<String> {
    table.get::<String>(key).ok()
}

/// A list of strings, or nothing. A missing key and an empty list mean the same thing to every
/// caller here, so they are not distinguished.
fn strings(table: &Table, key: &str) -> Vec<String> {
    let Ok(list) = table.get::<Table>(key) else {
        return Vec::new();
    };
    list.sequence_values::<String>()
        .filter_map(Result::ok)
        .collect()
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
    use dacode_input::{Action, Key, Match};
    use dacode_theme::Mode;

    fn run(source: &str) -> Config {
        eval(source, "test.lua")
    }

    #[test]
    fn transparency_is_a_percentage_and_never_a_vanishing_window() {
        assert_eq!(
            run("dacode.set { transparency = 20 }").transparency,
            Some(20)
        );
        assert_eq!(
            run("dacode.set { transparency = 400 }").transparency,
            Some(90),
            "a window that is all desktop has nothing left to edit in"
        );
        assert_eq!(
            run("dacode.set { transparency = -5 }").transparency,
            Some(0)
        );
    }

    #[test]
    fn animations_takes_one_of_three_names_and_reports_anything_else() {
        // A misspelt value must not quietly become "full": someone who typed "none" meant to switch
        // motion off, and an editor that carries on animating says nothing about why.
        let config = run("dacode.set { animations = \"reduced\" }");
        assert_eq!(config.animations, Some("reduced".into()));
        assert!(config.problems.is_empty(), "{:?}", config.problems);

        for bad in ["\"none\"", "false", "true", "0"] {
            let config = run(&format!("dacode.set {{ animations = {bad} }}"));
            assert_eq!(config.animations, None, "{bad}");
            assert!(
                config.problems.iter().any(|p| p.contains("animations")),
                "{bad} was not reported: {:?}",
                config.problems
            );
        }
        // Absent is not a problem: most configs never mention it.
        assert!(run("dacode.set { tab_width = 2 }").problems.is_empty());
    }

    #[test]
    fn a_configuration_written_for_vex_still_runs() {
        // Every init.lua written before the rename calls `vex.set`, including the block the
        // settings panel appends. Refusing them would reset everyone's settings on upgrade.
        let config = run("vex.set { tab_width = 3 }\nvex.map(\"normal\", \"x\", \"save\")");
        assert_eq!(config.tab_width, Some(3));
        assert!(config.problems.is_empty(), "{:?}", config.problems);
    }

    #[test]
    fn the_example_config_is_a_config_this_reader_accepts() {
        // `examples/init.lua` is what the documentation tells people to copy, and what the packages
        // install as `examples/init.lua` beside the README — and until now nothing read it. A typo in
        // an action name, an option renamed out from under it, or a stray comma would have been
        // found by the first person to follow the instructions rather than by the build.
        //
        // It is checked for *problems*, not for values: the file is meant to be edited, so what
        // matters is that every line in it still means something. `eval` collects rather than
        // throws, which is the whole reason a bad config cannot stop the editor opening.
        let config = eval(
            include_str!("../../../examples/init.lua"),
            "examples/init.lua",
        );
        assert!(
            config.problems.is_empty(),
            "the example config no longer loads cleanly: {:?}",
            config.problems
        );

        // A commented-out line teaches nothing about whether it would work, so the two that are
        // live are the ones worth pinning: the file would otherwise document `vim_keys` and a
        // leader without either being exercised.
        assert_eq!(config.modal, Some(true));
        assert_eq!(
            config
                .keymap
                .lookup(Mode::Normal, &Key::parse_sequence(" w")),
            Match::Exact(Action::Save),
            "the leader binding the file demonstrates does not resolve"
        );
    }

    #[test]
    fn a_byte_order_mark_does_not_reject_the_whole_file() {
        // Notepad and `Set-Content` both write one, so on Windows this is what an init.lua someone
        // edited normally actually looks like. Lua reports it as `unexpected symbol near '<\239>'`,
        // naming a byte the user cannot see in a file they will swear is correct — found by writing
        // a config from PowerShell and watching the editor refuse it.
        let config = run("\u{feff}dacode.set { tab_width = 3 }");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.tab_width, Some(3));
    }

    #[test]
    fn options_are_read() {
        let config = run(r##"
            dacode.set {
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
            dacode.set { leader = "," }
            dacode.map("normal", "<leader>w", "save")
            dacode.map("normal", "<leader>f", "search_project")
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
            dacode.theme {
              bg = "#101418",
              keyword = "oklch(0.7 0.16 318)",
            }
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        let mut theme = Theme::walnut();
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
              dacode.map("normal", pair[1], pair[2])
            end
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.keymap.len(), 2);
    }

    #[test]
    fn an_error_partway_through_keeps_what_already_ran() {
        let config = run(r##"
            dacode.set { tab_width = 8 }
            error("something went wrong")
            dacode.set { tab_width = 2 }
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
        let config = run("dacode.set { tab_width = }");

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
            dacode.map("normal", "<leader>x", "sav")
            dacode.theme { bakcground = "#000000" }
            dacode.theme { bg = "not a color" }
            dacode.set { appearance = "twilight" }
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

        let missing = load(Path::new("/nonexistent/dacode/init.lua"));
        assert!(
            missing.problems.is_empty(),
            "a missing file is not an error"
        );
        assert!(missing.is_default());
    }

    #[test]
    fn the_vocabulary_is_introspectable_from_lua() {
        // `dacode.actions` exists so a user can discover what they may bind without reading the source.
        let config = run(r##"
            local found = false
            for _, name in ipairs(dacode.actions) do
              if name == "goto_definition" then found = true end
            end
            if not found then error("goto_definition is not advertised") end
            if #dacode.tokens == 0 then error("no theme tokens advertised") end
        "##);
        assert!(config.problems.is_empty(), "{:?}", config.problems);
    }

    #[test]
    fn every_advertised_action_resolves_and_none_is_missing() {
        // The advertised list once lagged the vocabulary by every action added after it was
        // written; the two focus commands are the ones it was found missing.
        let names = action_names();
        for name in &names {
            assert!(Action::from_name(name).is_some(), "{name} is advertised");
        }
        for wanted in ["focus_explorer", "focus_editor"] {
            assert!(names.contains(&wanted), "{wanted} is not advertised");
        }
    }

    #[test]
    fn an_unmentioned_option_stays_unset() {
        // Lua has no distinction between false and absent when coerced, so reading a bool naively
        // made every `dacode.set` that omitted `soft_wrap` turn wrapping off.
        let config = run("dacode.set { tab_width = 2 }");
        assert_eq!(config.soft_wrap, None, "absent is not false");

        let explicit = run("dacode.set { soft_wrap = false }");
        assert_eq!(explicit.soft_wrap, Some(false));

        let on = run("dacode.set { soft_wrap = true }");
        assert_eq!(on.soft_wrap, Some(true));
    }

    #[test]
    fn a_typo_gets_a_suggestion_rather_than_a_catalogue() {
        // The message is shown in a box a few lines tall, so listing nineteen tokens is not an
        // answer. A one-character typo should be named.
        let config = run(r##"
            dacode.theme { bg_pannel = "#101418" }
            dacode.map("normal", "x", "sav")
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
    fn language_servers_can_be_named() {
        let config = run(r##"
            dacode.server("python", { command = ".venv/bin/pylsp" })
            dacode.server("zig", { command = "zls", args = { "--stdio" }, extensions = { "zig" } })
        "##);

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].language, "python");
        assert_eq!(config.servers[0].command, ".venv/bin/pylsp");
        assert!(
            config.servers[0].args.is_empty() && config.servers[0].extensions.is_empty(),
            "what was not said stays unsaid, so the caller can fill it from its own table"
        );
        assert_eq!(config.servers[1].args, vec!["--stdio"]);
        assert_eq!(config.servers[1].extensions, vec!["zig"]);
    }

    #[test]
    fn a_config_that_only_names_a_server_is_not_a_default_one() {
        // The trap this exists for: the caller skips a config that looks default, so leaving
        // `servers` out of that check would have the file read, parsed and then thrown away.
        let config = run(r##"dacode.server("python", { command = "pylsp" })"##);
        assert!(!config.is_default());
    }

    #[test]
    fn a_name_with_no_close_match_points_at_the_list() {
        // `backgruond` is a typo of `background`, which is not a DaCode token at all — DaCode calls it
        // `bg` — so there is nothing near enough to suggest.
        let config = run(r##"dacode.theme { backgruond = "#101418" }"##);
        assert_eq!(config.problems.len(), 1);
        assert!(
            config.problems[0].contains("dacode.tokens"),
            "{:?}",
            config.problems
        );
    }
}

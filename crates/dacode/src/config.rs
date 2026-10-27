//! Reading DaCode's own config, including anything the importer wrote.
//!
//! The parser is hand-rolled over the small subset the importer emits — `[section]` headers and
//! `key = value` lines. Pulling in a TOML crate plus serde derive for a hundred lines of key/value
//! reading would add compile time and binary size to an editor whose whole premise is staying
//! light. If config grows to need arrays-of-tables and nested inline structures, swap it then.
//!
//! **Lua is the real config** (see `dacode-lua`): `init.lua` is read after these files and wins where
//! both set something. This layer remains because it is what the VSCode importer *writes* — an
//! imported theme has to work without the user having hand-authored anything.

use dacode_input::Keymap;
use dacode_theme::{Appearance, Oklch, Theme, Typography};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A parsed config file: `section.key` → value.
#[derive(Debug, Default, Clone)]
pub struct Config {
    values: HashMap<String, String>,
}

impl Config {
    pub fn parse(source: &str) -> Self {
        let mut values = HashMap::new();
        let mut section = String::new();

        // The same byte-order mark `dacode_lua::eval` strips, for the same reason: an editor on Windows
        // writes one without saying so, and it would otherwise become part of the first section's
        // name — which fails silently here rather than loudly, since a section nobody asked for is
        // simply never read.
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);

        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_string();
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            // Trailing comments are stripped, but not from inside a quoted value.
            let value = value.trim();
            let value = if value.starts_with('"') {
                value
                    .strip_prefix('"')
                    .and_then(|v| v.split('"').next())
                    .unwrap_or(value)
                    .to_string()
            } else {
                value.split('#').next().unwrap_or(value).trim().to_string()
            };

            let key = key.trim();
            let full = if section.is_empty() {
                key.to_string()
            } else {
                format!("{section}.{key}")
            };
            values.insert(full, value);
        }

        Self { values }
    }

    pub fn read(path: &Path) -> Option<Self> {
        std::fs::read_to_string(path).ok().map(|s| Self::parse(&s))
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.string(key)?.parse().ok()
    }

    pub fn usize(&self, key: &str) -> Option<usize> {
        self.string(key)?.parse().ok()
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.string(key)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    /// Reads a color. `oklch(L C H)` is what the importer writes — a theme stays inspectable and
    /// hand-adjustable rather than being an opaque hex blob — but hex is accepted too, since the
    /// same parser serves hand-written files.
    pub fn color(&self, key: &str) -> Option<Oklch> {
        Oklch::parse(self.string(key)?)
    }
}

/// What the gutter counts.
///
/// Relative numbering is a *Vim* idea: it exists so `12j` is a thing you can read off the screen.
/// With the grammar off there is no count to type, and a column of numbers that are not the line's
/// number is one more thing that felt wrong without being nameable — so the default follows the
/// grammar rather than being one answer for both.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LineNumbers {
    Absolute,
    Relative,
    Off,
}

impl LineNumbers {
    pub const ALL: [LineNumbers; 3] = [
        LineNumbers::Absolute,
        LineNumbers::Relative,
        LineNumbers::Off,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            LineNumbers::Absolute => "absolute",
            LineNumbers::Relative => "relative",
            LineNumbers::Off => "off",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|value| value.name() == name.trim().to_ascii_lowercase())
    }

    /// What to use when nothing said: relative under Vim's grammar, plain numbers without it.
    pub fn default_for(modal: bool) -> Self {
        if modal {
            LineNumbers::Relative
        } else {
            LineNumbers::Absolute
        }
    }
}

/// Everything config can influence at startup.
#[derive(Debug, Clone)]
pub struct Settings {
    pub typography: Typography,
    pub theme: Theme,
    pub tab_width: usize,
    pub soft_wrap: bool,
    /// Reformat through the language server after every save.
    pub format_on_save: bool,
    /// Whether Vim's modal grammar is in force.
    pub modal: bool,
    /// Pin enclosing block heads above the viewport.
    pub sticky_scroll: bool,
    /// Ask the language server for inlay hints.
    pub inlay_hints: bool,
    /// Write the file a moment after typing stops.
    pub autosave: bool,
    /// How much of the desktop shows through the window, as a percentage. 0 is opaque.
    pub transparency: u8,
    /// How much the interface moves.
    pub animations: crate::motion::Animations,
    /// What the gutter counts.
    pub line_numbers: LineNumbers,
    /// Which theme file was loaded, for reporting.
    pub theme_source: Option<String>,
    /// User key bindings from `init.lua`.
    pub keymap: Keymap,
    /// Language servers named by `dacode.server(...)`, tried before the ones DaCode knows about.
    pub servers: Vec<dacode_lsp::Configured>,
    /// Formatters named by `dacode.formatter(...)`, likewise.
    pub formatters: Vec<crate::format::Configured>,
    /// Which options were set by something other than the built-in defaults, and by what.
    ///
    /// Recorded so the settings panel can say where a value came from. When a setting is not what you
    /// expect, which of three files last set it is the only useful question, and no editor answers it
    /// by showing you one of them.
    pub sources: std::collections::HashMap<String, crate::settings_ui::Source>,
    /// Everything wrong with the configuration, in the user's terms. Surfaced at startup rather
    /// than logged, because a binding that silently did not load is indistinguishable from a bug
    /// in the editor.
    pub problems: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            typography: Typography::default(),
            theme: Theme::slate(),
            tab_width: 4,
            soft_wrap: true,
            // Off by default: a formatter reflowing a file you did not ask it to is worse than no
            // formatter, and it is the kind of default that loses trust the first time it fires.
            format_on_save: false,
            modal: true,
            // Off by default: the pinned block heads read as a highlighted band at the top of the
            // code that followed the scroll, which looked like a selection nobody had made.
            sticky_scroll: false,
            inlay_hints: false,
            // Off by default for the same reason `format_on_save` is: writing someone's file
            // without being asked is not a decision to make on their behalf. It is one switch away
            // for the many people who have never pressed ctrl-s and should not lose work for it.
            autosave: false,
            // Opaque: see-through is a taste, and on a desktop without a compositor it is not
            // available at all.
            transparency: 0,
            animations: crate::motion::Animations::Full,
            line_numbers: LineNumbers::Relative,
            theme_source: None,
            keymap: Keymap::new(),
            servers: Vec::new(),
            formatters: Vec::new(),
            sources: std::collections::HashMap::new(),
            problems: Vec::new(),
        }
    }
}

/// Re-exported so callers do not have to know which module owns the platform rules.
pub fn config_dir() -> PathBuf {
    crate::paths::config_dir()
}

/// Loads settings, then a theme if one is named or a single theme file exists.
///
/// Missing config is not an error: the built-in defaults are the product's real defaults, and the
/// editor must start with no configuration at all.
pub fn load() -> Settings {
    load_from(&config_dir())
}

/// Loads from a specific directory.
///
/// The directory is a parameter rather than read from the environment inside, so tests can each use
/// their own — `DACODE_CONFIG_DIR` is process-wide, and tests that set it clobber each other when
/// cargo runs them in parallel. That lesson came from the recovery tests.
pub fn load_from(dir: &Path) -> Settings {
    let mut settings = Settings::default();
    // Tracked rather than derived from `theme_source`, which is only set for a theme that came from a
    // *file*: a built-in named here has no source, and reading its absence as "nothing chose a theme"
    // would let `appearance = "light"` overwrite the amoled the user asked for by name.
    let mut named_theme = false;

    // `imported-settings.toml` is what the importer writes; `config.toml` is the hand-written one,
    // and wins where both set the same key.
    for name in ["imported-settings.toml", "config.toml"] {
        let Some(config) = Config::read(&dir.join(name)) else {
            continue;
        };

        let imported = crate::settings_ui::Source::Imported;
        if let Some(size) = config.float("editor.font_size") {
            settings.typography = Typography::with_code_size(size);
            settings.sources.insert("font_size".into(), imported);
        }
        if let Some(width) = config.usize("editor.tab_width") {
            settings.tab_width = width.clamp(1, 16);
            settings.sources.insert("tab_width".into(), imported);
        }
        if let Some(wrap) = config.bool("editor.soft_wrap") {
            settings.soft_wrap = wrap;
            settings.sources.insert("soft_wrap".into(), imported);
        }
        if let Some(format) = config.bool("editor.format_on_save") {
            settings.format_on_save = format;
            settings.sources.insert("format_on_save".into(), imported);
        }

        // A named theme is a file or a built-in; `appearance` alone flips the built-in theme.
        if let Some(name) = config.string("theme.name") {
            if let Some(resolved) = resolve_theme(dir, name) {
                settings.theme = resolved.theme;
                settings.theme_source = resolved.file;
                named_theme = true;
            }
        }
        if !named_theme {
            if let Some("light") = config.string("theme.appearance") {
                settings.theme = Theme::linen();
            }
        }
    }

    apply_lua(dir, &mut settings);
    settings
}

/// Runs `init.lua` over whatever the TOML files set.
///
/// Lua last, and therefore Lua wins: the hand-written config is the user's intent, while the
/// imported files are a starting point they did not type.
fn apply_lua(dir: &Path, settings: &mut Settings) {
    let path = dir.join("init.lua");
    let config = dacode_lua::load(&path);
    if config.is_default() && config.problems.is_empty() {
        return;
    }

    let from_config = crate::settings_ui::Source::Config;
    if let Some(size) = config.font_size {
        settings.typography = Typography::with_code_size(size);
        settings.sources.insert("font_size".into(), from_config);
    }
    if let Some(width) = config.tab_width {
        settings.tab_width = width.clamp(1, 16);
        settings.sources.insert("tab_width".into(), from_config);
    }
    if let Some(wrap) = config.soft_wrap {
        settings.soft_wrap = wrap;
        settings.sources.insert("soft_wrap".into(), from_config);
    }
    if let Some(format) = config.format_on_save {
        settings.format_on_save = format;
        settings
            .sources
            .insert("format_on_save".into(), from_config);
    }
    if let Some(modal) = config.modal {
        settings.modal = modal;
        settings.sources.insert("vim_keys".into(), from_config);
    }
    if let Some(sticky) = config.sticky_scroll {
        settings.sticky_scroll = sticky;
        settings.sources.insert("sticky_scroll".into(), from_config);
    }
    if let Some(hints) = config.inlay_hints {
        settings.inlay_hints = hints;
        settings.sources.insert("inlay_hints".into(), from_config);
    }
    if let Some(autosave) = config.autosave {
        settings.autosave = autosave;
        settings.sources.insert("autosave".into(), from_config);
    }
    if let Some(percent) = config.transparency {
        settings.transparency = percent.min(90);
        settings.sources.insert("transparency".into(), from_config);
    }
    if let Some(name) = &config.animations {
        // `dacode-lua` has already refused a bad spelling and said so; this only fails if the two lists
        // of names have drifted apart, which a test in this file rules out.
        if let Some(animations) = crate::motion::Animations::from_name(name) {
            settings.animations = animations;
            settings.sources.insert("animations".into(), from_config);
        }
    }
    // The gutter's default follows the grammar, so it has to be settled *after* `vim_keys` — which
    // is why this is not a plain `unwrap_or`.
    settings.line_numbers = match config.line_numbers.as_deref().map(LineNumbers::parse) {
        Some(Some(numbers)) => {
            settings.sources.insert("line_numbers".into(), from_config);
            numbers
        }
        Some(None) => {
            settings
                .problems
                .push("line_numbers must be \"absolute\", \"relative\" or \"off\"".into());
            LineNumbers::default_for(settings.modal)
        }
        None => LineNumbers::default_for(settings.modal),
    };

    // A named theme replaces the whole theme; `appearance` alone flips the built-in one. Either way
    // this happens before `paint`, so individual token overrides land on top of the right base.
    if let Some(name) = &config.theme_name {
        match resolve_theme(dir, name) {
            Some(resolved) => {
                settings.theme = resolved.theme;
                settings.theme_source = resolved.file;
                settings.sources.insert("theme".into(), from_config);
            }
            None => settings.problems.push(format!(
                "theme `{name}` is not one of {} and there is no file at {}",
                built_in_names(),
                theme_path(dir, name).display()
            )),
        }
    } else if let Some(appearance) = config.appearance {
        settings.theme = Theme::for_appearance(appearance);
        settings.theme_source = None;
        settings.sources.insert("appearance".into(), from_config);
    }

    // Validated here rather than in `dacode-lua`, which does not know what languages exist — the same
    // split as theme names, and for the same reason: the crate that owns the table is the one that
    // can say what is wrong with a name in the user's terms.
    for server in &config.servers {
        match dacode_lsp::configure(
            &server.language,
            &server.command,
            &server.args,
            &server.extensions,
        ) {
            Ok(configured) => settings.servers.push(configured),
            Err(problem) => settings.problems.push(problem),
        }
    }
    if !settings.servers.is_empty() {
        settings.sources.insert("servers".into(), from_config);
    }

    for formatter in &config.formatters {
        match crate::format::configure(
            &formatter.language,
            &formatter.command,
            &formatter.args,
            &formatter.extensions,
        ) {
            Ok(configured) => settings.formatters.push(configured),
            Err(problem) => settings.problems.push(problem),
        }
    }
    if !settings.formatters.is_empty() {
        settings.sources.insert("formatters".into(), from_config);
    }

    config.paint(&mut settings.theme);
    settings.keymap = config.keymap;
    settings.problems.extend(config.problems);
}

/// A theme found by name, and the file it came from if it came from one.
struct Resolved {
    theme: Theme,
    file: Option<String>,
}

/// A theme by the name written in config: a file in `themes/`, or one of the built-ins.
///
/// The file is tried first, so a `themes/dark.toml` the user wrote shadows the built-in of that name —
/// their file is the more specific answer, and silently preferring the compiled-in one would leave
/// them editing a file that never loads.
///
/// Built-ins were missing from this lookup, which made the settings panel contradict itself: choosing
/// a theme there writes `theme = "amoled"` into `init.lua`, and the next start reported it as a theme
/// that could not be found. The panel's own output was rejected by the reader that wrote it.
fn resolve_theme(dir: &Path, name: &str) -> Option<Resolved> {
    let path = theme_path(dir, name);
    if let Some(theme) = load_theme(&path) {
        return Some(Resolved {
            theme,
            file: Some(path.display().to_string()),
        });
    }

    Theme::by_name(name).map(|theme| Resolved { theme, file: None })
}

/// The built-in names, for the message a bad theme name earns. Read from `Theme::BUILT_IN` so a theme
/// added there cannot go unmentioned here.
fn built_in_names() -> String {
    Theme::BUILT_IN
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a named theme lives. A name is slugified rather than used verbatim, so `theme = "One Dark
/// Pro"` finds the file the importer wrote as `one-dark-pro.toml`.
fn theme_path(dir: &Path, name: &str) -> PathBuf {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    dir.join("themes")
        .join(format!("{}.toml", slug.trim_matches('-')))
}

/// Loads a theme file, keeping built-in values for anything it does not set.
pub fn load_theme(path: &Path) -> Option<Theme> {
    let config = Config::read(path)?;

    // A file that does not say is classified by its canvas, before anything is built on it: the
    // base supplies every token the file leaves out, so a white `bg` over the dark base was a white
    // page with dark-theme ink, and `toggle_theme` went the wrong way from it.
    let canvas = ["theme.bg", "theme.surfaces.bg"]
        .iter()
        .find_map(|key| config.color(key));
    let appearance = match config.string("theme.appearance") {
        Some("light") => Appearance::Light,
        Some("dark") => Appearance::Dark,
        _ => canvas.map_or(Appearance::Dark, Appearance::of_background),
    };
    let mut theme = Theme::for_appearance(appearance);

    // The token names come from `Theme::set_token`, so this reader, the Lua config and the importer
    // cannot drift apart on what a theme may set.
    for token in Theme::TOKENS {
        // Both spellings are accepted: `[theme.syntax] keyword = …` flattens to one and the
        // importer's grouped surfaces to the other.
        for key in [
            format!("theme.{token}"),
            format!("theme.syntax.{token}"),
            format!("theme.surfaces.{token}"),
            format!("theme.lines.{token}"),
            format!("theme.text.{token}"),
            format!("theme.overlays.{token}"),
        ] {
            if let Some(color) = config.color(&key) {
                theme.set_token(token, color);
                break;
            }
        }
    }

    Some(theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_and_keys_parse() {
        let config = Config::parse(
            "# a comment\n\
             [editor]\n\
             font_size = 15\n\
             soft_wrap = true\n\
             font_family = \"JetBrains Mono\"\n\
             \n\
             [theme]\n\
             name = \"One Dark Pro\"\n",
        );

        assert_eq!(config.float("editor.font_size"), Some(15.0));
        assert_eq!(config.bool("editor.soft_wrap"), Some(true));
        assert_eq!(config.string("editor.font_family"), Some("JetBrains Mono"));
        assert_eq!(config.string("theme.name"), Some("One Dark Pro"));
        assert_eq!(config.string("missing.key"), None);
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_section() {
        // Without stripping it the first section is named "\u{feff}editor", which nothing ever asks
        // for — so an imported config edited on Windows loses everything above its second heading and
        // says nothing about it.
        let config = Config::parse("\u{feff}[editor]\ntab_width = 2\n");
        assert_eq!(config.usize("editor.tab_width"), Some(2));
    }

    #[test]
    fn oklch_colors_parse() {
        let config = Config::parse("[theme.syntax]\nkeyword = \"oklch(0.694 0.164 318.2)\"\n");
        let color = config.color("theme.syntax.keyword").expect("parses");
        assert!((color.l - 0.694).abs() < 0.001);
        assert!((color.c - 0.164).abs() < 0.001);
        assert!((color.h - 318.2).abs() < 0.1);
    }

    #[test]
    fn trailing_comments_do_not_corrupt_values() {
        let config = Config::parse("[editor]\ntab_width = 2 # two spaces\n");
        assert_eq!(config.usize("editor.tab_width"), Some(2));
    }

    #[test]
    fn a_theme_file_keeps_builtin_values_for_what_it_omits() {
        // A theme that sets only a background must not black out everything else.
        let dir = std::env::temp_dir().join(format!("dacode-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.toml");
        std::fs::write(
            &path,
            "[theme]\nappearance = \"dark\"\n\n[theme.surfaces]\nbg = \"oklch(0.293 0.016 264.3)\"\n",
        )
        .unwrap();

        let theme = load_theme(&path).expect("loads");
        assert!(
            (theme.bg.l - 0.293).abs() < 0.001,
            "background came from the file"
        );
        assert_eq!(
            theme.syntax.keyword.to_hex(),
            Theme::walnut().syntax.keyword.to_hex(),
            "unset colors keep the built-in value"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A config directory that only this test uses, so no two tests can interfere.
    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dacode-cfg-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lua_config_reaches_settings() {
        let dir = scratch("lua");
        std::fs::write(
            dir.join("init.lua"),
            "dacode.set { font_size = 16, tab_width = 2, soft_wrap = false }\n\
             dacode.theme { bg = \"#101418\" }\n\
             dacode.map(\"normal\", \"<leader>w\", \"save\")\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert_eq!(settings.tab_width, 2);
        assert!(!settings.soft_wrap);
        assert_eq!(settings.theme.bg.to_hex(), "#101418");
        assert_eq!(settings.keymap.len(), 1);
        assert!((settings.typography.code_size - 16.0).abs() < 0.01);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lua_sets_the_animations_and_every_name_the_enum_prints_is_accepted() {
        // `dacode-lua` cannot see `Animations`, so it carries its own list of spellings. Round-tripping
        // every variant through the real reader is what stops the two lists drifting apart.
        for animations in crate::motion::Animations::ALL {
            let dir = scratch(&format!("animations-{}", animations.name()));
            std::fs::write(
                dir.join("init.lua"),
                format!("dacode.set {{ animations = \"{}\" }}\n", animations.name()),
            )
            .unwrap();

            let settings = load_from(&dir);
            assert!(settings.problems.is_empty(), "{:?}", settings.problems);
            assert_eq!(settings.animations, animations);
            assert_eq!(
                settings.sources.get("animations"),
                Some(&crate::settings_ui::Source::Config)
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn animations_default_to_full_when_nothing_says_otherwise() {
        let dir = scratch("animations-default");
        assert_eq!(load_from(&dir).animations, crate::motion::Animations::Full);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lua_wins_over_the_imported_toml() {
        // The TOML is what the VSCode importer wrote; init.lua is what the user typed. When they
        // disagree, the thing the user typed is the answer.
        let dir = scratch("precedence");
        std::fs::write(
            dir.join("imported-settings.toml"),
            "[editor]\ntab_width = 8\nsoft_wrap = true\n",
        )
        .unwrap();
        std::fs::write(dir.join("init.lua"), "dacode.set { tab_width = 3 }\n").unwrap();

        let settings = load_from(&dir);
        assert_eq!(settings.tab_width, 3, "Lua wins");
        assert!(
            settings.soft_wrap,
            "what Lua did not set still comes from the imported file"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_lua_config_still_starts_the_editor() {
        let dir = scratch("broken");
        std::fs::write(
            dir.join("init.lua"),
            "dacode.set { tab_width = 2 }\ndacode.map(\"normal\", \"x\", \"nonsense\")\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert_eq!(settings.tab_width, 2, "the good half applied");
        assert_eq!(settings.problems.len(), 1);
        assert!(settings.keymap.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_config_directory_at_all_yields_defaults() {
        let settings = load_from(Path::new("/nonexistent/dacode-config"));
        assert!(settings.problems.is_empty());
        assert_eq!(settings.tab_width, 4);
        assert!(settings.soft_wrap);
        assert!(settings.keymap.is_empty());
    }

    #[test]
    fn a_lua_theme_name_loads_the_file_the_importer_wrote() {
        let dir = scratch("named-theme");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(
            dir.join("themes/one-dark-pro.toml"),
            "[theme]\nappearance = \"dark\"\n\n[theme.syntax]\nkeyword = \"oklch(0.7 0.16 318)\"\n",
        )
        .unwrap();
        // Written with spaces and capitals, as a VSCode theme is actually named.
        std::fs::write(
            dir.join("init.lua"),
            "dacode.set { theme = \"One Dark Pro\" }\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert!(settings.theme_source.is_some(), "the file was found");
        assert!((settings.theme.syntax.keyword.h - 318.0).abs() < 0.5);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_theme_the_settings_tab_can_write_loads_again() {
        // The panel writes `theme = "<built-in name>"` into init.lua, so every one of those names has
        // to come back as a theme and not as a problem. It did not: the reader looked names up as
        // files only, and choosing a theme in the panel meant an error box on the next start.
        for (name, _, build) in Theme::BUILT_IN {
            let dir = scratch(&format!("built-in-{name}"));
            std::fs::write(
                dir.join("init.lua"),
                format!("dacode.set {{ theme = \"{name}\" }}\n"),
            )
            .unwrap();

            let settings = load_from(&dir);
            assert!(
                settings.problems.is_empty(),
                "{name}: {:?}",
                settings.problems
            );
            assert_eq!(settings.theme.bg.to_hex(), build().bg.to_hex(), "{name}");
            // By name too: the colour alone could not tell a lookup that returned a different theme
            // on the same canvas.
            assert_eq!(settings.theme.name(), Some(name), "{name}");
            assert!(
                settings.theme_source.is_none(),
                "{name} is compiled in, not a file"
            );

            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn an_old_theme_name_in_init_lua_still_starts_the_theme_it_was_renamed_to() {
        // The themes were renamed for their palettes, and the settings tab had been writing the old
        // names into init.lua for a release. Through the real reader, because it is the reader that
        // would have reported "no such theme" and started the fallback.
        for (old, new) in Theme::ALIASES {
            let dir = scratch(&format!("old-theme-{old}"));
            std::fs::write(
                dir.join("init.lua"),
                format!("dacode.set {{ theme = \"{old}\" }}\n"),
            )
            .unwrap();
            let settings = load_from(&dir);
            assert!(
                settings.problems.is_empty(),
                "{old}: {:?}",
                settings.problems
            );
            assert_eq!(settings.theme.name(), Some(new), "{old}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn a_theme_file_that_does_not_say_is_dark_or_light_by_its_canvas() {
        // The settings tab files themes under Dark and Light, and a file with a paper-white `bg`
        // and no `appearance` line used to come back Dark — built over the dark base, so every
        // token it did not set was a dark theme's, and the toggle went the wrong way from it.
        let dir = scratch("appearance-by-canvas");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("themes/paper.toml"), "[theme]\nbg = \"#fbfaf7\"\n").unwrap();
        std::fs::write(dir.join("themes/ink.toml"), "[theme]\nbg = \"#101010\"\n").unwrap();
        std::fs::write(
            dir.join("themes/declared.toml"),
            "[theme]\nappearance = \"dark\"\nbg = \"#fbfaf7\"\n",
        )
        .unwrap();

        let paper = load_theme(&dir.join("themes/paper.toml")).unwrap();
        assert_eq!(paper.appearance, Appearance::Light);
        assert_eq!(
            paper.ink.to_hex(),
            Theme::linen().ink.to_hex(),
            "the tokens it leaves out come from the light base"
        );
        let ink = load_theme(&dir.join("themes/ink.toml")).unwrap();
        assert_eq!(ink.appearance, Appearance::Dark);
        // A file that says is taken at its word.
        let declared = load_theme(&dir.join("themes/declared.toml")).unwrap();
        assert_eq!(declared.appearance, Appearance::Dark);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_theme_file_shadows_the_built_in_of_the_same_name() {
        // Someone who writes themes/dark.toml means it; preferring the compiled-in one would leave
        // them editing a file that never loads.
        let dir = scratch("shadow");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(
            dir.join("themes/dark.toml"),
            "[theme]\nbg = \"oklch(0.2 0.02 250)\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("init.lua"), "dacode.set { theme = \"dark\" }\n").unwrap();

        let settings = load_from(&dir);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert!(settings.theme_source.is_some(), "the file won");
        assert_ne!(settings.theme.bg.to_hex(), Theme::walnut().bg.to_hex());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_named_language_server_reaches_the_settings() {
        let dir = scratch("servers");
        std::fs::write(
            dir.join("init.lua"),
            "dacode.server(\"python\", { command = \".venv/bin/pylsp\" })\n\
             dacode.server(\"zig\", { command = \"zls\", extensions = { \"zig\" } })\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert_eq!(settings.servers.len(), 2);
        assert_eq!(settings.servers[0].spec.program, ".venv/bin/pylsp");
        assert_eq!(
            settings.servers[0].extensions,
            vec!["py", "pyi"],
            "dacode's own table fills in what config did not say"
        );
        assert_eq!(
            settings.sources.get("servers"),
            Some(&crate::settings_ui::Source::Config)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_server_dacode_cannot_make_sense_of_is_reported_and_the_rest_still_loads() {
        // The same rule the whole config layer follows: an editor that refuses to start because of
        // one bad line is an editor you cannot use to fix that line.
        let dir = scratch("bad-server");
        std::fs::write(
            dir.join("init.lua"),
            "dacode.set { tab_width = 2 }\n\
             dacode.server(\"zig\", { command = \"zls\" })\n\
             dacode.server(\"go\", { command = \"gopls\" })\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert_eq!(settings.tab_width, 2, "the good half applied");
        assert_eq!(settings.problems.len(), 1);
        assert!(
            settings.problems[0].contains("extensions"),
            "{:?}",
            settings.problems
        );
        assert_eq!(settings.servers.len(), 1, "and the valid one still loaded");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_named_formatter_reaches_the_settings() {
        let dir = scratch("formatters");
        std::fs::write(
            dir.join("init.lua"),
            "dacode.formatter(\"python\", { command = \".venv/bin/black\", args = { \"-\" } })\n\
             dacode.formatter(\"nix\", { command = \"nixfmt\", extensions = { \"nix\" } })\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert_eq!(settings.formatters.len(), 2);
        assert_eq!(settings.formatters[0].formatter.program, ".venv/bin/black");
        assert_eq!(
            settings.formatters[0].extensions,
            vec!["py", "pyi"],
            "dacode's own table fills in what config did not say"
        );
        assert_eq!(settings.formatters[1].extensions, vec!["nix"]);
        assert_eq!(
            settings.sources.get("formatters"),
            Some(&crate::settings_ui::Source::Config)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_named_theme_is_reported_rather_than_silently_ignored() {
        let dir = scratch("missing-theme");
        std::fs::write(dir.join("init.lua"), "dacode.set { theme = \"nope\" }\n").unwrap();

        let settings = load_from(&dir);
        assert_eq!(settings.problems.len(), 1);
        assert!(
            settings.problems[0].contains("nope"),
            "{:?}",
            settings.problems
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}

//! Reading vex's own config, including anything the importer wrote.
//!
//! The parser is hand-rolled over the small subset the importer emits — `[section]` headers and
//! `key = value` lines. Pulling in a TOML crate plus serde derive for a hundred lines of key/value
//! reading would add compile time and binary size to an editor whose whole premise is staying
//! light. If config grows to need arrays-of-tables and nested inline structures, swap it then.
//!
//! **Lua is the real config** (see `vex-lua`): `init.lua` is read after these files and wins where
//! both set something. This layer remains because it is what the VSCode importer *writes* — an
//! imported theme has to work without the user having hand-authored anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use vex_input::Keymap;
use vex_theme::{Appearance, Oklch, Theme, Typography};

/// A parsed config file: `section.key` → value.
#[derive(Debug, Default, Clone)]
pub struct Config {
    values: HashMap<String, String>,
}

impl Config {
    pub fn parse(source: &str) -> Self {
        let mut values = HashMap::new();
        let mut section = String::new();

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

/// Everything config can influence at startup.
#[derive(Debug, Clone)]
pub struct Settings {
    pub typography: Typography,
    pub theme: Theme,
    pub tab_width: usize,
    pub soft_wrap: bool,
    /// Which theme file was loaded, for reporting.
    pub theme_source: Option<String>,
    /// User key bindings from `init.lua`.
    pub keymap: Keymap,
    /// Everything wrong with the configuration, in the user's terms. Surfaced at startup rather
    /// than logged, because a binding that silently did not load is indistinguishable from a bug
    /// in the editor.
    pub problems: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            typography: Typography::default(),
            theme: Theme::dark(),
            tab_width: 4,
            soft_wrap: true,
            theme_source: None,
            keymap: Keymap::new(),
            problems: Vec::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("VEX_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".config/vex")
        })
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
/// their own — `VEX_CONFIG_DIR` is process-wide, and tests that set it clobber each other when
/// cargo runs them in parallel. That lesson came from the recovery tests.
pub fn load_from(dir: &Path) -> Settings {
    let mut settings = Settings::default();

    // `imported-settings.toml` is what the importer writes; `config.toml` is the hand-written one,
    // and wins where both set the same key.
    for name in ["imported-settings.toml", "config.toml"] {
        let Some(config) = Config::read(&dir.join(name)) else {
            continue;
        };

        if let Some(size) = config.float("editor.font_size") {
            settings.typography = Typography::with_code_size(size);
        }
        if let Some(width) = config.usize("editor.tab_width") {
            settings.tab_width = width.clamp(1, 16);
        }
        if let Some(wrap) = config.bool("editor.soft_wrap") {
            settings.soft_wrap = wrap;
        }

        // A named theme is looked up as a file; `appearance` alone flips the built-in theme.
        if let Some(name) = config.string("theme.name") {
            let path = theme_path(dir, name);
            if let Some(theme) = load_theme(&path) {
                settings.theme = theme;
                settings.theme_source = Some(path.display().to_string());
            }
        }
        if settings.theme_source.is_none() {
            if let Some("light") = config.string("theme.appearance") {
                settings.theme = Theme::light();
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
    let config = vex_lua::load(&path);
    if config.is_default() && config.problems.is_empty() {
        return;
    }

    if let Some(size) = config.font_size {
        settings.typography = Typography::with_code_size(size);
    }
    if let Some(width) = config.tab_width {
        settings.tab_width = width.clamp(1, 16);
    }
    if let Some(wrap) = config.soft_wrap {
        settings.soft_wrap = wrap;
    }

    // A named theme replaces the whole theme; `appearance` alone flips the built-in one. Either way
    // this happens before `paint`, so individual token overrides land on top of the right base.
    if let Some(name) = &config.theme_name {
        let path = theme_path(dir, name);
        match load_theme(&path) {
            Some(theme) => {
                settings.theme = theme;
                settings.theme_source = Some(path.display().to_string());
            }
            None => settings
                .problems
                .push(format!("theme `{name}` not found at {}", path.display())),
        }
    } else if let Some(appearance) = config.appearance {
        settings.theme = Theme::for_appearance(appearance);
        settings.theme_source = None;
    }

    config.paint(&mut settings.theme);
    settings.keymap = config.keymap;
    settings.problems.extend(config.problems);
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

    let appearance = match config.string("theme.appearance") {
        Some("light") => Appearance::Light,
        _ => Appearance::Dark,
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
        let dir = std::env::temp_dir().join(format!("vex-config-{}", std::process::id()));
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
            Theme::dark().syntax.keyword.to_hex(),
            "unset colors keep the built-in value"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A config directory that only this test uses, so no two tests can interfere.
    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-cfg-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lua_config_reaches_settings() {
        let dir = scratch("lua");
        std::fs::write(
            dir.join("init.lua"),
            "vex.set { font_size = 16, tab_width = 2, soft_wrap = false }\n\
             vex.theme { bg = \"#101418\" }\n\
             vex.map(\"normal\", \"<leader>w\", \"save\")\n",
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
    fn lua_wins_over_the_imported_toml() {
        // The TOML is what the VSCode importer wrote; init.lua is what the user typed. When they
        // disagree, the thing the user typed is the answer.
        let dir = scratch("precedence");
        std::fs::write(
            dir.join("imported-settings.toml"),
            "[editor]\ntab_width = 8\nsoft_wrap = true\n",
        )
        .unwrap();
        std::fs::write(dir.join("init.lua"), "vex.set { tab_width = 3 }\n").unwrap();

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
            "vex.set { tab_width = 2 }\nvex.map(\"normal\", \"x\", \"nonsense\")\n",
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
        let settings = load_from(Path::new("/nonexistent/vex-config"));
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
            "vex.set { theme = \"One Dark Pro\" }\n",
        )
        .unwrap();

        let settings = load_from(&dir);
        assert!(settings.problems.is_empty(), "{:?}", settings.problems);
        assert!(settings.theme_source.is_some(), "the file was found");
        assert!((settings.theme.syntax.keyword.h - 318.0).abs() < 0.5);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_named_theme_is_reported_rather_than_silently_ignored() {
        let dir = scratch("missing-theme");
        std::fs::write(dir.join("init.lua"), "vex.set { theme = \"nope\" }\n").unwrap();

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

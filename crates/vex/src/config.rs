//! Reading vex's own config, including anything the importer wrote.
//!
//! The parser is hand-rolled over the small subset the importer emits — `[section]` headers and
//! `key = value` lines. Pulling in a TOML crate plus serde derive for a hundred lines of key/value
//! reading would add compile time and binary size to an editor whose whole premise is staying
//! light. If config grows to need arrays-of-tables and nested inline structures, swap it then.
//!
//! Lua config (from the locked decisions) supersedes this and can read the same values; this is
//! what makes an imported theme usable *today*.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

    /// Reads an `oklch(L C H)` value — the form the importer writes, so a theme stays inspectable
    /// and hand-adjustable rather than being an opaque hex blob.
    pub fn color(&self, key: &str) -> Option<Oklch> {
        let raw = self.string(key)?;
        let inner = raw.trim().strip_prefix("oklch(")?.strip_suffix(')')?;
        let mut parts = inner.split_whitespace();
        let l = parts.next()?.parse().ok()?;
        let c = parts.next()?.parse().ok()?;
        let h = parts.next()?.parse().ok()?;
        Some(Oklch::new(l, c, h))
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            typography: Typography::default(),
            theme: Theme::dark(),
            tab_width: 4,
            soft_wrap: true,
            theme_source: None,
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
    let dir = config_dir();
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
            let path = dir
                .join("themes")
                .join(format!("{}.toml", slug.trim_matches('-')));
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

    settings
}

/// Loads a theme file, keeping built-in values for anything it does not set.
pub fn load_theme(path: &Path) -> Option<Theme> {
    let config = Config::read(path)?;

    let appearance = match config.string("theme.appearance") {
        Some("light") => Appearance::Light,
        _ => Appearance::Dark,
    };
    let mut theme = Theme::for_appearance(appearance);

    let set = |key: &str, target: &mut Oklch| {
        if let Some(color) = config.color(key) {
            *target = color;
        }
    };

    set("theme.surfaces.bg", &mut theme.bg);
    set("theme.surfaces.bg_alt", &mut theme.bg_alt);
    set("theme.surfaces.bg_panel", &mut theme.bg_panel);
    set("theme.surfaces.row_active", &mut theme.row_active);
    set("theme.lines.rule", &mut theme.rule);
    set("theme.lines.rule_soft", &mut theme.rule_soft);
    set("theme.text.ink", &mut theme.ink);
    set("theme.text.ink_dim", &mut theme.ink_dim);
    set("theme.text.ink_faint", &mut theme.ink_faint);
    set("theme.syntax.keyword", &mut theme.syntax.keyword);
    set("theme.syntax.type", &mut theme.syntax.type_);
    set("theme.syntax.function", &mut theme.syntax.function);
    set("theme.syntax.attribute", &mut theme.syntax.attribute);
    set("theme.syntax.comment", &mut theme.syntax.comment);
    set("theme.syntax.string", &mut theme.syntax.string);
    set("theme.syntax.number", &mut theme.syntax.number);
    set("theme.syntax.constant", &mut theme.syntax.constant);

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
}

//! Reading colour schemes in whatever format they arrive in.
//!
//! People do not have "a VSCode theme"; they have *a theme*, in whichever format the place they found
//! it uses. Four are common enough to be worth reading, and all four are text:
//!
//! - **VSCode JSON** (`.json`) — the richest, and the only one that names editor chrome as well as
//!   syntax. Handled in `theme.rs`, which this module defers to.
//! - **TextMate** (`.tmTheme`) — a plist. Sublime and older TextMate themes, and the format most
//!   "download a theme" pages still serve.
//! - **base16 / base24** (`.yaml`, `.yml`) — sixteen or twenty-four numbered colours, no semantics.
//!   Hundreds of schemes exist in it precisely because it is trivial.
//! - **vex TOML** — what the importer itself writes, so an exported theme can be read back.
//!
//! Parsed by hand rather than with a plist and a YAML crate. Both formats are used here in one narrow
//! shape — a flat map of names to hex strings — and two more dependencies with their own error types
//! to read that would cost more than the fifty lines below. What is *not* attempted is the general
//! case: a `.tmTheme` with nested dictionaries per scope selector is read for its top-level colours
//! and its syntax scopes, and anything cleverer is ignored rather than guessed at.

use crate::plan::ImportReport;
use std::collections::BTreeMap;
use vex_theme::{Appearance, Oklch, Theme};

/// The formats that can be read.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Format {
    VscodeJson,
    TextMate,
    Base16,
    VexToml,
}

impl Format {
    /// The format a file name implies, if any.
    pub fn of(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        // `.tmTheme` is checked before the generic extensions because some ship as `name.tmTheme.xml`.
        if lower.ends_with(".tmtheme") || lower.ends_with(".tmtheme.xml") {
            return Some(Format::TextMate);
        }
        match lower.rsplit('.').next()? {
            "json" | "jsonc" => Some(Format::VscodeJson),
            "yaml" | "yml" => Some(Format::Base16),
            "toml" => Some(Format::VexToml),
            "plist" | "xml" => Some(Format::TextMate),
            _ => None,
        }
    }

    /// Every extension the importer accepts, for a message that has to list them.
    pub const EXTENSIONS: &'static str = ".json, .jsonc, .tmTheme, .plist, .yaml, .yml, .toml";
}

/// Reads a colour scheme from text, in whichever of the formats it is.
///
/// Returns the theme and how many colours were recognised — a scheme that yielded two is a scheme that
/// was not really understood, and the caller says so rather than presenting a mostly-default theme as
/// an import.
pub fn read(name: &str, text: &str, report: &mut ImportReport) -> Option<(Theme, usize)> {
    let format = Format::of(name).or_else(|| {
        // No usable extension: guess from the content, which is unambiguous for these four.
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            Some(Format::VscodeJson)
        } else if trimmed.starts_with("<?xml") || trimmed.starts_with("<plist") {
            Some(Format::TextMate)
        } else if trimmed.contains("base00") {
            Some(Format::Base16)
        } else if trimmed.starts_with('[') || trimmed.contains("[theme") {
            Some(Format::VexToml)
        } else {
            None
        }
    })?;

    match format {
        Format::VscodeJson => {
            let json = crate::parse_jsonc(text).ok()?;
            Some(crate::theme::import_theme(&json, report))
        }
        Format::TextMate => from_pairs(&text_mate_pairs(text), report),
        Format::Base16 => from_pairs(&base16_pairs(text), report),
        Format::VexToml => from_pairs(&toml_pairs(text), report),
    }
}

/// Colour names to hex, from a TextMate plist.
///
/// The shape that matters: `<key>settings</key>` holds one dictionary of editor colours (no `name`) and
/// then one per scope. Both are read as flat key/value pairs, with scope selectors mapped onto the
/// roles vex has.
fn text_mate_pairs(text: &str) -> BTreeMap<String, String> {
    let mut pairs = BTreeMap::new();
    let mut key: Option<String> = None;
    let mut scope: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();

        if let Some(value) = tag_body(line, "key") {
            key = Some(value.to_string());
            continue;
        }
        if let Some(value) = tag_body(line, "string") {
            match key.take().as_deref() {
                // A scope selector names what the *next* `foreground` applies to.
                Some("scope") => scope = Some(value.to_string()),
                Some("foreground") => {
                    let role = match &scope {
                        Some(selector) => role_for_scope(selector),
                        // No scope yet: this is the editor's own foreground.
                        None => Some("foreground"),
                    };
                    if let Some(role) = role {
                        pairs.entry(role.to_string()).or_insert(value.to_string());
                    }
                }
                Some(name) => {
                    pairs.insert(name.to_string(), value.to_string());
                }
                None => {}
            }
            continue;
        }
        // A dictionary ends, so the scope it belonged to is over.
        if line == "</dict>" {
            scope = None;
        }
    }

    pairs
}

/// The body of a one-line XML tag: `<key>name</key>` gives `name`.
fn tag_body<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = line.strip_prefix(&open)?;
    start.strip_suffix(&close)
}

/// Which vex role a TextMate scope selector is about.
///
/// Only the selectors that map cleanly. A theme's `meta.function-call.python` is not worth guessing at,
/// and a wrong guess is worse than the default it would replace.
fn role_for_scope(selector: &str) -> Option<&'static str> {
    let selector = selector.to_ascii_lowercase();
    let first = |needle: &str| selector.contains(needle);

    if first("comment") {
        Some("comment")
    } else if first("string") {
        Some("string")
    } else if first("constant.numeric") || first("constant.language") {
        Some("number")
    } else if first("keyword") || first("storage.modifier") {
        Some("keyword")
    } else if first("entity.name.function") || first("support.function") {
        Some("function")
    } else if first("entity.name.type")
        || first("entity.name.class")
        || first("support.type")
        || first("storage.type")
    {
        Some("type")
    } else if first("variable.parameter") || first("entity.other.attribute") {
        Some("attribute")
    } else if first("constant") {
        Some("constant")
    } else {
        None
    }
}

/// `base00: "#181818"` lines, quoted or not.
fn base16_pairs(text: &str) -> BTreeMap<String, String> {
    let mut pairs = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        if key.starts_with("base") && !value.is_empty() {
            // base16 hex is often written without the `#`.
            let value = if value.starts_with('#') {
                value.to_string()
            } else {
                format!("#{value}")
            };
            pairs.insert(key.to_string(), value);
        }
    }
    pairs
}

/// The importer's own TOML, read back.
fn toml_pairs(text: &str) -> BTreeMap<String, String> {
    let mut pairs = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                pairs.insert(key.trim().to_string(), value.to_string());
            }
        }
    }
    pairs
}

/// Builds a theme from name/colour pairs, whichever format they came from.
///
/// The mapping is deliberately conservative: what is recognised is set, and what is not keeps the
/// built-in value. A theme is a relationship between colours, and half-inventing the missing half
/// produces something that belongs to nobody.
fn from_pairs(
    pairs: &BTreeMap<String, String>,
    report: &mut ImportReport,
) -> Option<(Theme, usize)> {
    if pairs.is_empty() {
        return None;
    }

    let colour = |names: &[&str]| -> Option<Oklch> {
        names
            .iter()
            .find_map(|name| pairs.get(*name))
            .and_then(|value| Oklch::parse(value))
    };

    // base16's numbered slots have fixed meanings, which is the whole point of the format:
    // base00 background … base05 foreground, base08-0F the accents.
    let background = colour(&["background", "base00", "bg", "theme.surfaces.bg"])?;
    let foreground = colour(&["foreground", "base05", "ink", "theme.text.ink"]).unwrap_or(
        if background.l < 0.5 {
            Oklch::new(0.9, 0.01, 80.0)
        } else {
            Oklch::new(0.26, 0.015, 70.0)
        },
    );

    let appearance = if background.l < 0.5 {
        Appearance::Dark
    } else {
        Appearance::Light
    };
    let mut theme = Theme::for_appearance(appearance);
    let mut recognised = 0;

    let mut set = |token: &str, value: Option<Oklch>, recognised: &mut usize| {
        if let Some(value) = value {
            if theme.set_token(token, value) {
                *recognised += 1;
            }
        }
    };

    set("bg", Some(background), &mut recognised);
    set("ink", Some(foreground), &mut recognised);
    set(
        "bg_alt",
        colour(&["base01", "lineHighlight", "theme.surfaces.bg_alt"]),
        &mut recognised,
    );
    set(
        "bg_panel",
        colour(&["base01", "theme.surfaces.bg_panel"]),
        &mut recognised,
    );
    set(
        "row_active",
        colour(&["lineHighlight", "base02", "theme.surfaces.row_active"]),
        &mut recognised,
    );
    set(
        "rule",
        colour(&["base03", "theme.lines.rule"]),
        &mut recognised,
    );
    set(
        "ink_dim",
        colour(&["base04", "theme.text.ink_dim"]),
        &mut recognised,
    );
    set(
        "ink_faint",
        colour(&["base03", "theme.text.ink_faint"]),
        &mut recognised,
    );
    set(
        "comment",
        colour(&["comment", "base03", "theme.syntax.comment"]),
        &mut recognised,
    );
    set(
        "string",
        colour(&["string", "base0B", "theme.syntax.string"]),
        &mut recognised,
    );
    set(
        "keyword",
        colour(&["keyword", "base0E", "theme.syntax.keyword"]),
        &mut recognised,
    );
    set(
        "function",
        colour(&["function", "base0D", "theme.syntax.function"]),
        &mut recognised,
    );
    set(
        "type",
        colour(&["type", "base0A", "theme.syntax.type"]),
        &mut recognised,
    );
    set(
        "number",
        colour(&["number", "base09", "theme.syntax.number"]),
        &mut recognised,
    );
    set(
        "constant",
        colour(&["constant", "base08", "theme.syntax.constant"]),
        &mut recognised,
    );
    set(
        "attribute",
        colour(&["attribute", "base0C", "theme.syntax.attribute"]),
        &mut recognised,
    );

    if recognised < 4 {
        report.note(format!(
            "only {recognised} colours were recognised — that is not enough to call it an import"
        ));
        return None;
    }

    Some((theme, recognised))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ImportReport {
        ImportReport::default()
    }

    #[test]
    fn formats_are_recognised_by_extension() {
        assert_eq!(Format::of("one-dark.json"), Some(Format::VscodeJson));
        assert_eq!(Format::of("Solarized.tmTheme"), Some(Format::TextMate));
        assert_eq!(Format::of("gruvbox.yaml"), Some(Format::Base16));
        assert_eq!(Format::of("mine.toml"), Some(Format::VexToml));
        // Some tmThemes ship with a second extension.
        assert_eq!(Format::of("Old.tmTheme.xml"), Some(Format::TextMate));
        assert_eq!(Format::of("notes.txt"), None);
    }

    #[test]
    fn a_base16_scheme_maps_its_numbered_slots() {
        // The format's slots have fixed meanings, which is why hundreds of schemes exist in it.
        let yaml = "\
scheme: \"Gruvbox dark\"
base00: \"282828\"
base01: \"3c3836\"
base03: \"665c54\"
base05: \"d5c4a1\"
base08: \"fb4934\"
base09: \"fe8019\"
base0A: \"fabd2f\"
base0B: \"b8bb26\"
base0D: \"83a598\"
base0E: \"d3869b\"
";
        let (theme, recognised) = read("gruvbox.yaml", yaml, &mut report()).expect("reads");

        assert!(recognised >= 8, "recognised only {recognised}");
        assert_eq!(theme.bg.to_hex(), "#282828");
        assert_eq!(theme.ink.to_hex(), "#d5c4a1");
        assert_eq!(
            theme.syntax.keyword.to_hex(),
            "#d3869b",
            "base0E is keywords"
        );
        assert_eq!(theme.syntax.string.to_hex(), "#b8bb26", "base0B is strings");
        assert_eq!(theme.appearance, vex_theme::Appearance::Dark);
    }

    #[test]
    fn hex_without_a_hash_is_still_hex() {
        // base16 files usually omit it, and refusing them over a missing character would be silly.
        let yaml =
            "base00: 1d2021\nbase05: ebdbb2\nbase08: fb4934\nbase0B: b8bb26\nbase0D: 83a598\n";
        let (theme, _) = read("x.yaml", yaml, &mut report()).expect("reads");
        assert_eq!(theme.bg.to_hex(), "#1d2021");
    }

    #[test]
    fn a_textmate_theme_maps_its_scopes() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>name</key>
  <string>Sample</string>
  <key>settings</key>
  <array>
    <dict>
      <key>settings</key>
      <dict>
        <key>background</key>
        <string>#1B1D1E</string>
        <key>foreground</key>
        <string>#F8F8F2</string>
        <key>lineHighlight</key>
        <string>#3E3D32</string>
      </dict>
    </dict>
    <dict>
      <key>scope</key>
      <string>comment</string>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#75715E</string>
      </dict>
    </dict>
    <dict>
      <key>scope</key>
      <string>keyword, storage.modifier</string>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#F92672</string>
      </dict>
    </dict>
    <dict>
      <key>scope</key>
      <string>string</string>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#E6DB74</string>
      </dict>
    </dict>
    <dict>
      <key>scope</key>
      <string>entity.name.function</string>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#A6E22E</string>
      </dict>
    </dict>
  </array>
</dict>
</plist>
"#;
        let (theme, recognised) = read("Sample.tmTheme", plist, &mut report()).expect("reads");

        assert!(recognised >= 5, "recognised only {recognised}");
        assert_eq!(theme.bg.to_hex(), "#1b1d1e");
        assert_eq!(theme.ink.to_hex(), "#f8f8f2");
        assert_eq!(theme.syntax.comment.to_hex(), "#75715e");
        assert_eq!(theme.syntax.keyword.to_hex(), "#f92672");
        assert_eq!(theme.syntax.string.to_hex(), "#e6db74");
        assert_eq!(theme.syntax.function.to_hex(), "#a6e22e");
    }

    #[test]
    fn the_importers_own_toml_reads_back() {
        // What `vex import --theme` writes has to be readable by the same importer, or exporting and
        // reimporting a theme loses it.
        let toml = "\
[theme]
appearance = \"dark\"

[theme.surfaces]
bg = \"oklch(0.213 0.026 291.1)\"

[theme.text]
ink = \"oklch(0.909 0.03 290)\"

[theme.syntax]
keyword = \"oklch(0.528 0.079 227.7)\"
string = \"oklch(0.843 0.11 74.6)\"
comment = \"oklch(0.538 0.044 291.4)\"
function = \"oklch(0.836 0.054 21.1)\"
";
        let (theme, recognised) = read("mine.toml", toml, &mut report()).expect("reads");
        assert!(recognised >= 5);
        assert!((theme.bg.l - 0.213).abs() < 0.01);
    }

    #[test]
    fn a_light_background_makes_a_light_theme() {
        // Inferred from the background rather than trusted from a label: a "dark" theme with a white
        // background is a mislabelled theme, and the background is the honest signal.
        let yaml = "base00: \"fbf1c7\"\nbase05: \"3c3836\"\nbase08: \"9d0006\"\nbase0B: \"79740e\"\nbase0D: \"076678\"\n";
        let (theme, _) = read("light.yaml", yaml, &mut report()).expect("reads");
        assert_eq!(theme.appearance, vex_theme::Appearance::Light);
    }

    #[test]
    fn a_file_with_almost_nothing_in_it_is_refused() {
        // Better than presenting a mostly-default theme as an imported one.
        let mut report = report();
        assert!(read("thin.yaml", "base00: \"000000\"\n", &mut report).is_none());
        assert!(
            report.render().contains("recognised"),
            "and it says why: {}",
            report.render()
        );

        assert!(read("empty.yaml", "", &mut report).is_none());
        assert!(read("junk.txt", "not a theme at all", &mut report).is_none());
    }

    #[test]
    fn the_format_is_guessed_from_content_when_the_name_says_nothing() {
        let yaml = "base00: \"282828\"\nbase05: \"ebdbb2\"\nbase08: \"fb4934\"\nbase0B: \"b8bb26\"\nbase0D: \"83a598\"\n";
        assert!(read("downloaded-file", yaml, &mut report()).is_some());
    }
}

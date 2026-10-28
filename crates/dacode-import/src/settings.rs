//! `settings.json` → DaCode settings.
//!
//! Only keys with a genuine DaCode equivalent are mapped. Everything else is *reported*, not dropped
//! silently: a setting that vanished without explanation looks like a bug, while one listed as
//! "no equivalent" is a decision the user can see.

use crate::plan::ImportReport;
use serde_json::Value;

/// Editor preferences DaCode understands. Every field is optional: absent means "VSCode did not set
/// this", which is different from "VSCode set it to the default".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedSettings {
    pub code_size: Option<f32>,
    pub font_family: Option<String>,
    pub tab_width: Option<usize>,
    /// `editor.insertSpaces`
    pub insert_spaces: Option<bool>,
    pub soft_wrap: Option<bool>,
    /// `editor.rulers` — the first one becomes the column guide.
    pub ruler_column: Option<usize>,
    /// `editor.lineNumbers`: "relative" maps onto DaCode's default numbering.
    pub relative_line_numbers: Option<bool>,
    pub format_on_save: Option<bool>,
    /// `workbench.colorTheme`, so the matching theme can be looked up separately.
    pub theme_name: Option<String>,
    /// True when the theme name suggests a light theme.
    pub prefers_light: Option<bool>,
}

impl ImportedSettings {
    /// How many settings actually came across.
    pub fn count(&self) -> usize {
        [
            self.code_size.is_some(),
            self.font_family.is_some(),
            self.tab_width.is_some(),
            self.insert_spaces.is_some(),
            self.soft_wrap.is_some(),
            self.ruler_column.is_some(),
            self.relative_line_numbers.is_some(),
            self.format_on_save.is_some(),
            self.theme_name.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count()
    }

    /// Renders as DaCode config, so the import is reviewable before it is adopted.
    pub fn to_config(&self) -> String {
        let mut out = String::from("# Imported from VSCode settings.json\n\n[editor]\n");

        if let Some(size) = self.code_size {
            out.push_str(&format!("font_size = {size}\n"));
        }
        if let Some(family) = &self.font_family {
            out.push_str(&format!("font_family = {family:?}\n"));
        }
        if let Some(width) = self.tab_width {
            out.push_str(&format!("tab_width = {width}\n"));
        }
        if let Some(spaces) = self.insert_spaces {
            out.push_str(&format!("insert_spaces = {spaces}\n"));
        }
        if let Some(wrap) = self.soft_wrap {
            out.push_str(&format!("soft_wrap = {wrap}\n"));
        }
        if let Some(column) = self.ruler_column {
            out.push_str(&format!("ruler_column = {column}\n"));
        }
        if let Some(relative) = self.relative_line_numbers {
            out.push_str(&format!("relative_line_numbers = {relative}\n"));
        }
        if let Some(format) = self.format_on_save {
            out.push_str(&format!("format_on_save = {format}\n"));
        }
        if let Some(theme) = &self.theme_name {
            out.push_str(&format!("\n[theme]\nname = {theme:?}\n"));
            if let Some(light) = self.prefers_light {
                out.push_str(&format!(
                    "appearance = {:?}\n",
                    if light { "light" } else { "dark" }
                ));
            }
        }

        out
    }
}

/// Keys DaCode deliberately ignores, with the reason. Listed so the report can be specific instead of
/// lumping everything into "unknown".
const KNOWN_UNSUPPORTED: &[(&str, &str)] = &[
    ("editor.minimap.enabled", "DaCode has no minimap"),
    ("editor.codeLens", "needs LSP code lenses (M6)"),
    ("editor.inlineSuggest.enabled", "no inline suggestions"),
    (
        "workbench.iconTheme",
        "DaCode uses no icons — every element is text or a rectangle",
    ),
    (
        "editor.cursorBlinking",
        "cursor blink is defined by the mode, not by a setting",
    ),
    (
        "editor.fontLigatures",
        "ligatures follow the font, and are always on when it has them",
    ),
    (
        "terminal.integrated.shell",
        "the terminal starts $SHELL, or PowerShell on Windows; choosing another is not a setting yet",
    ),
];

pub fn import_settings(json: &Value, report: &mut ImportReport) -> ImportedSettings {
    let mut settings = ImportedSettings::default();
    let Some(map) = json.as_object() else {
        report.note("settings.json is not a JSON object");
        return settings;
    };

    settings.code_size = map
        .get("editor.fontSize")
        .and_then(number)
        .map(|n| n as f32);
    settings.font_family = map
        .get("editor.fontFamily")
        .and_then(Value::as_str)
        // VSCode allows a CSS-style fallback list; DaCode resolves families itself, so take the first.
        .and_then(|list| list.split(',').next())
        .map(|family| {
            family
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
                .to_string()
        });
    settings.tab_width = map
        .get("editor.tabSize")
        .and_then(number)
        .map(|n| n as usize);
    settings.insert_spaces = map.get("editor.insertSpaces").and_then(Value::as_bool);
    settings.format_on_save = map.get("editor.formatOnSave").and_then(Value::as_bool);

    // `wordWrap` is an enum, not a boolean: "off" | "on" | "wordWrapColumn" | "bounded".
    settings.soft_wrap = map
        .get("editor.wordWrap")
        .and_then(Value::as_str)
        .map(|mode| mode != "off");

    settings.ruler_column = map
        .get("editor.rulers")
        .and_then(Value::as_array)
        .and_then(|rulers| rulers.first())
        .and_then(|first| {
            // Entries are either a number or an object with a column and a color.
            number(first).or_else(|| first.get("column").and_then(number))
        })
        .map(|n| n as usize);

    settings.relative_line_numbers = map
        .get("editor.lineNumbers")
        .and_then(Value::as_str)
        .map(|mode| mode == "relative");

    if let Some(theme) = map.get("workbench.colorTheme").and_then(Value::as_str) {
        settings.theme_name = Some(theme.to_string());
        let lowered = theme.to_lowercase();
        settings.prefers_light = Some(lowered.contains("light"));
    }

    // Report the notable omissions by name.
    for (key, reason) in KNOWN_UNSUPPORTED {
        if map.contains_key(*key) {
            report.note(format!("{key}: {reason}"));
        }
    }

    let recognized = [
        "editor.fontSize",
        "editor.fontFamily",
        "editor.tabSize",
        "editor.insertSpaces",
        "editor.formatOnSave",
        "editor.wordWrap",
        "editor.rulers",
        "editor.lineNumbers",
        "workbench.colorTheme",
    ];
    let unmapped = map
        .keys()
        .filter(|key| {
            !recognized.contains(&key.as_str())
                && !KNOWN_UNSUPPORTED.iter().any(|(known, _)| known == key)
        })
        .count();
    if unmapped > 0 {
        report.note(format!(
            "{unmapped} further settings had no DaCode equivalent and were left alone"
        ));
    }

    settings
}

/// VSCode writes numbers as either integers or floats; accept both.
fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_i64().map(|n| n as f64))
}

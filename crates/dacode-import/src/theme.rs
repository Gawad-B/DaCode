//! VSCode color themes → DaCode's palette.
//!
//! A VSCode theme is two things: `colors` (UI element keys) and `tokenColors` (TextMate scopes).
//! Both are mapped onto DaCode's small role set. The conversion goes through OKLCH rather than
//! carrying hex around, so an imported theme joins the same system as a native one — its accents
//! can be checked for legibility and re-lightened for a light background.
//!
//! Missing keys fall back to the built-in theme rather than to black: a theme that only sets a
//! background should still be usable.

use crate::plan::ImportReport;
use dacode_theme::{Appearance, Oklch, Theme};
use serde_json::Value;

/// Reads a theme, returning the converted theme and how many colors were recognized.
pub fn import_theme(json: &Value, report: &mut ImportReport) -> (Theme, usize) {
    // `type` declares light or dark; without it, infer from the background's lightness — which is
    // the honest signal anyway, since a "dark" theme with a white background is a mislabeled theme.
    let declared = json.get("type").and_then(Value::as_str);
    let colors = json.get("colors").and_then(Value::as_object);

    let background = colors
        .and_then(|c| c.get("editor.background"))
        .and_then(Value::as_str)
        .and_then(Oklch::from_hex);

    let appearance = match (declared, background) {
        (Some("light"), _) => Appearance::Light,
        (Some("dark"), _) => Appearance::Dark,
        (_, Some(bg)) if bg.l > 0.5 => Appearance::Light,
        _ => Appearance::Dark,
    };

    let mut theme = Theme::for_appearance(appearance);
    let mut recognized = 0usize;

    let mut take = |key: &str, target: &mut Oklch| {
        if let Some(color) = colors
            .and_then(|c| c.get(key))
            .and_then(Value::as_str)
            .and_then(Oklch::from_hex)
        {
            // Themes use alpha for overlays; DaCode composites tints itself, so keep the hue and drop
            // the transparency.
            *target = color.with_alpha(1.0);
            recognized += 1;
        }
    };

    take("editor.background", &mut theme.bg);
    take("sideBar.background", &mut theme.bg_panel);
    take("statusBar.background", &mut theme.bg_alt);
    take("editor.foreground", &mut theme.ink);
    take("descriptionForeground", &mut theme.ink_dim);
    take("editorLineNumber.foreground", &mut theme.ink_faint);
    take("panel.border", &mut theme.rule);
    take("editorIndentGuide.background", &mut theme.rule_soft);
    take("list.activeSelectionBackground", &mut theme.row_active);

    // Token colors are matched by TextMate scope. Scopes are hierarchical, so the *most specific*
    // match wins — `keyword.control` beats a bare `keyword`.
    let mut syntax_hits = 0usize;
    if let Some(tokens) = json.get("tokenColors").and_then(Value::as_array) {
        let mut assign = |scopes: &[&str], target: &mut Oklch| {
            if let Some(color) = best_scope_match(tokens, scopes) {
                *target = color;
                syntax_hits += 1;
            }
        };

        assign(
            &["keyword", "storage", "keyword.control"],
            &mut theme.syntax.keyword,
        );
        assign(
            &["entity.name.type", "support.type", "storage.type"],
            &mut theme.syntax.type_,
        );
        assign(
            &["entity.name.function", "support.function"],
            &mut theme.syntax.function,
        );
        assign(
            &["entity.other.attribute-name", "meta.decorator"],
            &mut theme.syntax.attribute,
        );
        assign(&["comment"], &mut theme.syntax.comment);
        assign(&["string"], &mut theme.syntax.string);
        assign(&["constant.numeric"], &mut theme.syntax.number);
        assign(
            &["constant.language", "constant", "variable.other.constant"],
            &mut theme.syntax.constant,
        );
    }
    recognized += syntax_hits;

    if recognized == 0 {
        report.note("no recognizable colors — is this a VSCode theme file?");
    } else if syntax_hits == 0 {
        report.note("theme had no tokenColors; syntax colors kept from the built-in palette");
    }

    // A theme that leaves text illegible against its own background is worse than no import, so
    // the result is checked rather than trusted.
    let contrast = (theme.ink.l - theme.bg.l).abs();
    if contrast < 0.3 {
        report.note(format!(
            "low contrast after import (text {:.2} vs background {:.2}) — text lightness was adjusted",
            theme.ink.l, theme.bg.l
        ));
        theme.ink = if theme.bg.l > 0.5 {
            theme.ink.with_lightness(0.25)
        } else {
            theme.ink.with_lightness(0.9)
        };
    }

    (theme, recognized)
}

/// The color of the longest scope that matches one of `wanted`.
fn best_scope_match(tokens: &[Value], wanted: &[&str]) -> Option<Oklch> {
    let mut best: Option<(usize, Oklch)> = None;

    for token in tokens {
        let color = token
            .get("settings")
            .and_then(|s| s.get("foreground"))
            .and_then(Value::as_str)
            .and_then(Oklch::from_hex);
        let Some(color) = color else { continue };

        // `scope` is a string or an array of strings.
        let scopes: Vec<&str> = match token.get("scope") {
            Some(Value::String(scope)) => scope.split(',').map(str::trim).collect(),
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };

        for scope in scopes {
            for target in wanted {
                // A theme scope matches if it is a prefix of the target or vice versa: themes are
                // written at whatever specificity the author chose.
                if scope == *target || scope.starts_with(target) || target.starts_with(scope) {
                    let specificity = scope.len();
                    if best.map(|(len, _)| specificity > len).unwrap_or(true) {
                        best = Some((specificity, color.with_alpha(1.0)));
                    }
                }
            }
        }
    }

    best.map(|(_, color)| color)
}

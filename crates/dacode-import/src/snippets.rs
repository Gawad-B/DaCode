//! VSCode snippets → DaCode snippets.
//!
//! The format is straightforward; the substance is the placeholder syntax (`$1`, `${2:name}`,
//! `$0`), which needs an expansion engine in the editor. That engine does not exist yet, so
//! importing parses and stores snippets and the report says plainly that expansion is pending —
//! rather than importing them and letting them silently do nothing.

use crate::plan::ImportReport;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Snippet {
    pub name: String,
    /// What the user types to trigger it.
    pub prefix: String,
    /// The expansion, with placeholders intact.
    pub body: String,
    pub description: Option<String>,
    /// Tab stops found in the body, `$0` last. Empty means a plain text snippet.
    pub tab_stops: Vec<u32>,
}

impl Snippet {
    pub fn has_placeholders(&self) -> bool {
        !self.tab_stops.is_empty()
    }
}

/// Finds `$1` / `${1:default}` / `$0` tab stops, in order, deduplicated.
fn tab_stops(body: &str) -> Vec<u32> {
    let mut found = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '$' {
            let mut cursor = index + 1;
            if cursor < chars.len() && chars[cursor] == '{' {
                cursor += 1;
            }
            let start = cursor;
            while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if cursor > start {
                let number: String = chars[start..cursor].iter().collect();
                if let Ok(stop) = number.parse::<u32>() {
                    if !found.contains(&stop) {
                        found.push(stop);
                    }
                }
                index = cursor;
                continue;
            }
        }
        index += 1;
    }

    // `$0` is the final cursor position, so it sorts last however it was written.
    found.sort_by_key(|stop| if *stop == 0 { u32::MAX } else { *stop });
    found
}

pub fn import_snippets(json: &Value, report: &mut ImportReport) -> Vec<Snippet> {
    let Some(map) = json.as_object() else {
        report.note("snippet file is not a JSON object");
        return Vec::new();
    };

    let mut snippets = Vec::new();

    for (name, definition) in map {
        let Some(prefix) = definition.get("prefix").and_then(Value::as_str) else {
            continue;
        };

        // `body` is a string or an array of lines.
        let body = match definition.get("body") {
            Some(Value::String(body)) => body.clone(),
            Some(Value::Array(lines)) => lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };

        snippets.push(Snippet {
            name: name.clone(),
            prefix: prefix.to_string(),
            body: body.clone(),
            description: definition
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            tab_stops: tab_stops(&body),
        });
    }

    let with_placeholders = snippets.iter().filter(|s| s.has_placeholders()).count();
    if with_placeholders > 0 {
        report.note(format!(
            "{with_placeholders} snippets use tab stops; expansion is not implemented yet, so they \
             are stored but inert"
        ));
    }

    snippets.sort_by(|a, b| a.prefix.cmp(&b.prefix));
    snippets
}

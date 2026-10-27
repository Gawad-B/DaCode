//! Every character the interface itself draws is one the bundled Fira Code has.
//!
//! A character the face lacks is drawn from whatever installed font has it, so a label holding one is
//! a word with a letter in another typeface — and only on a machine with fonts installed, since the
//! screenshots run with `DACODE_NO_SYSTEM_FONTS` and draw a blank there instead. The user's own file
//! is exempt and should be: a CJK comment or an emoji in a string is better drawn in some face than
//! not at all. These are the editor's words.

use dacode::menu;
use dacode::settings_ui::{self, Category, Field, Importable, Link};
use dacode_input::commands::{COMMANDS, Group};
use dacode_render::fira_code_draws;
use std::path::{Path, PathBuf};

/// The characters of `text` the bundled face cannot draw. Control characters are layout, not glyphs.
fn undrawable(text: &str) -> Vec<char> {
    text.chars()
        .filter(|ch| !ch.is_control() && !fira_code_draws(*ch))
        .collect()
}

#[test]
fn the_tables_the_interface_reads_are_all_in_the_bundled_face() {
    // The command table feeds the palette, the help panel, the menus and the which-key popup, and the
    // settings tables feed every row of the settings tab: the strings most read, checked through the
    // functions that hand them to the screen, so a label built at runtime is checked as it is shown.
    let mut shown: Vec<String> = Vec::new();
    for command in COMMANDS {
        shown.push(command.what.to_string());
        shown.push(command.shown(true));
        shown.push(command.shown(false));
        shown.extend(menu::note(command.name).map(str::to_string));
    }
    shown.extend(Group::ALL.iter().map(|group| group.title().to_string()));
    shown.extend(menu::TITLES.iter().map(|title| title.to_string()));
    for category in Category::ALL {
        shown.push(category.title().to_string());
        for section in settings_ui::sections(category) {
            shown.push(section.title.to_string());
        }
    }
    for field in Field::ALL {
        for modal in [true, false] {
            shown.push(field.label(modal).to_string());
            shown.extend(field.explanation(modal).map(str::to_string));
        }
    }
    shown.extend(Importable::ALL.iter().map(|item| item.name().to_string()));
    // The theme dropdown's headings and rows, and the wizard's cards: "Rosé Pine" has an é.
    shown.extend(
        dacode_theme::Appearance::ALL
            .iter()
            .map(|appearance| appearance.title().to_string()),
    );
    shown.extend(
        dacode_theme::Theme::BUILT_IN
            .iter()
            .map(|(_, title, _)| title.to_string()),
    );
    shown.push(Link::RebindKeys.label().to_string());
    shown.push(Link::RebindKeys.explanation().to_string());

    let offenders: Vec<String> = shown
        .iter()
        .filter_map(|text| {
            let missing = undrawable(text);
            (!missing.is_empty()).then(|| format!("{missing:?} in {text:?}"))
        })
        .collect();
    assert!(shown.len() > 300, "only {} strings checked", shown.len());
    assert!(
        offenders.is_empty(),
        "drawn from another font, since Fira Code has no glyph for them:\n{}",
        offenders.join("\n")
    );
}

/// The crates whose strings reach the screen: the binary, the command table, the theme names, and
/// the libraries whose messages the status line repeats. The text-handling crates below them are
/// left out on purpose — a Unicode table in `dacode-text` is data about text, not text shown.
const SHOWN_FROM: [&str; 7] = [
    "dacode",
    "dacode-input",
    "dacode-theme",
    "dacode-lsp",
    "dacode-lua",
    "dacode-import",
    "dacode-project",
];

#[test]
fn every_literal_the_interface_draws_is_in_the_bundled_face() {
    // The tables above are the strings with names; this is everything else — the help panel's and the
    // wizard's prose, the status line's messages, the chips and glyphs `chrome.rs` and `layout.rs`
    // write inline. A source scan rather than a list, because a list is the thing that goes stale.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for krate in SHOWN_FROM {
        collect_rust(
            &workspace.join("crates").join(krate).join("src"),
            &mut files,
        );
    }

    let mut literals = 0;
    let mut seen_separator = false;
    let mut offenders = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        for literal in literals_outside_tests(&source) {
            literals += 1;
            seen_separator |= literal.contains('›');
            let missing = undrawable(&literal);
            if !missing.is_empty() {
                let shown = file.strip_prefix(&workspace).unwrap_or(file);
                offenders.push(format!("{}: {missing:?} in {literal:?}", shown.display()));
            }
        }
    }

    // A scan that read nothing passes. The first version of the `canonicalize` guard did exactly that.
    assert!(files.len() > 50, "scanned only {} files", files.len());
    assert!(literals > 2000, "found only {literals} literals");
    // The settings breadcrumb's separator, built at runtime from a literal: proof the scan reaches
    // the non-ASCII characters this guard exists for, not only the ASCII that cannot fail.
    assert!(
        seen_separator,
        "the scan never saw the › the settings page draws"
    );
    assert!(
        offenders.is_empty(),
        "drawn from another font, since Fira Code has no glyph for them:\n{}",
        offenders.join("\n")
    );
}

fn collect_rust(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The decoded contents of every string and char literal in `source`, outside comments and outside
/// anything marked `#[cfg(test)]`.
///
/// A small lexer rather than a regex, because the cases a regex gets wrong are the ordinary ones
/// here: a `'` that opens a lifetime, a `"` inside a raw string, a `//` inside a URL in a string.
/// Tests are skipped because they hold text the interface never shows — some of it chosen precisely
/// because no font has it. Byte strings are skipped as bytes, not text.
fn literals_outside_tests(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // When set, the brace depth at which a `#[cfg(test)]` item ends; literals inside are not kept.
    let mut skipping: Option<usize> = None;
    let mut pending_test = false;
    let mut depth = 0usize;
    let at = |i: usize, text: &str| {
        text.chars()
            .enumerate()
            .all(|(k, c)| chars.get(i + k) == Some(&c))
    };

    while i < chars.len() {
        let c = chars[i];
        if at(i, "//") {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if at(i, "/*") {
            let mut nest = 0;
            while i < chars.len() {
                if at(i, "/*") {
                    nest += 1;
                    i += 2;
                } else if at(i, "*/") {
                    nest -= 1;
                    i += 2;
                    if nest == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if at(i, "#[cfg(test)]") {
            pending_test = skipping.is_none();
            i += "#[cfg(test)]".len();
        } else if c == '{' {
            if pending_test {
                skipping = Some(depth);
                pending_test = false;
            }
            depth += 1;
            i += 1;
        } else if c == '}' {
            depth = depth.saturating_sub(1);
            if skipping == Some(depth) {
                skipping = None;
            }
            i += 1;
        } else if c == ';' {
            // `#[cfg(test)] use x;` — an item with no body.
            pending_test = false;
            i += 1;
        } else if c == '"' || ((c == 'r' || c == 'b') && starts_string(&chars, i)) {
            let previous = i.checked_sub(1).map(|p| chars[p]);
            let prefixed = previous.is_some_and(|p| p.is_alphanumeric() || p == '_');
            let (text, end, is_text) = if prefixed {
                // Part of an identifier, such as the `r` of `for` — never a literal's prefix.
                (String::new(), i + 1, false)
            } else {
                string_literal(&chars, i)
            };
            if is_text && skipping.is_none() {
                out.push(text);
            }
            i = end;
        } else if c == '\'' {
            match char_literal(&chars, i) {
                Some((text, end)) => {
                    if skipping.is_none() {
                        out.push(text);
                    }
                    i = end;
                }
                // A lifetime or a label.
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Whether the `r` or `b` at `i` opens a string: `r"`, `r#"`, `b"`, `br"`, `br#"`.
fn starts_string(chars: &[char], i: usize) -> bool {
    let mut j = i;
    if chars.get(j) == Some(&'b') {
        j += 1;
    }
    if chars.get(j) == Some(&'r') {
        j += 1;
        while chars.get(j) == Some(&'#') {
            j += 1;
        }
    }
    j > i && chars.get(j) == Some(&'"')
}

/// The literal starting at `i`, the index just past it, and whether it is text (not bytes).
fn string_literal(chars: &[char], i: usize) -> (String, usize, bool) {
    let mut j = i;
    let bytes = chars[j] == 'b';
    if bytes {
        j += 1;
    }
    if chars[j] == 'r' {
        j += 1;
        let mut hashes = 0;
        while chars[j] == '#' {
            hashes += 1;
            j += 1;
        }
        j += 1;
        let start = j;
        loop {
            if j >= chars.len() {
                return (String::new(), j, false);
            }
            if chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) {
                let text: String = chars[start..j].iter().collect();
                return (text, j + 1 + hashes, !bytes);
            }
            j += 1;
        }
    }
    j += 1;
    let mut text = String::new();
    while j < chars.len() && chars[j] != '"' {
        if chars[j] == '\\' {
            let (decoded, next) = escape(chars, j);
            text.extend(decoded);
            j = next;
        } else {
            text.push(chars[j]);
            j += 1;
        }
    }
    (text, j + 1, !bytes)
}

/// A char literal at `i`, or `None` for a lifetime.
fn char_literal(chars: &[char], i: usize) -> Option<(String, usize)> {
    match chars.get(i + 1)? {
        '\\' => {
            let (decoded, next) = escape(chars, i + 1);
            (chars.get(next) == Some(&'\'')).then(|| (decoded.into_iter().collect(), next + 1))
        }
        &ch => (chars.get(i + 2) == Some(&'\'')).then(|| (ch.to_string(), i + 3)),
    }
}

/// The character an escape at `i` stands for, and the index after it.
fn escape(chars: &[char], i: usize) -> (Option<char>, usize) {
    match chars.get(i + 1) {
        Some('n') => (Some('\n'), i + 2),
        Some('r') => (Some('\r'), i + 2),
        Some('t') => (Some('\t'), i + 2),
        Some('0') => (Some('\0'), i + 2),
        Some('x') => {
            let hex: String = chars[i + 2..i + 4].iter().collect();
            (
                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32),
                i + 4,
            )
        }
        Some('u') => {
            let close = (i..chars.len()).find(|&k| chars[k] == '}').unwrap_or(i + 2);
            let hex: String = chars[i + 3..close].iter().collect();
            (
                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32),
                close + 1,
            )
        }
        // A line continuation: the newline and the indentation after it are not part of the string.
        Some('\n') => {
            let mut j = i + 1;
            while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                j += 1;
            }
            (None, j)
        }
        Some(&other) => (Some(other), i + 2),
        None => (None, i + 1),
    }
}

#[test]
fn the_literal_scan_reads_what_the_compiler_would() {
    // The scan is only a guard if it finds the literals and only them.
    let source = r####"
        // "in a comment ★"
        /* "in a block ★" */
        fn f<'a>(x: &'a str) -> &'a str { let c = '›'; let e = '\u{2192}'; x }
        const S: &str = "a \"quoted\" ↑ and \u{2014} \
                         continued";
        const R: &str = r#"raw "★" inside"#;
        const B: &[u8] = b"bytes \xff";
        #[cfg(test)]
        mod tests { const T: &str = "test only ★"; }
        const AFTER: &str = "after";
    "####;
    let found = literals_outside_tests(source);
    assert_eq!(
        found,
        [
            "›",
            "→",
            "a \"quoted\" ↑ and — continued",
            "raw \"★\" inside",
            "after"
        ]
    );
}

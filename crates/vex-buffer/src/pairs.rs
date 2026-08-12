//! Bracket pairing and indentation.
//!
//! # Why this is lexical rather than syntax-aware
//!
//! Indentation has to be decided **on the keystroke**, and the syntax tree deliberately lags: it is
//! computed off-thread against a snapshot, and by §1.1 the UI thread never waits for it. Worse, at
//! the instant Enter is pressed the line being opened does not exist in any tree yet.
//!
//! So the rules here read the text directly — the previous line's indentation, its last meaningful
//! character, and the delimiters around the cursor. That is bounded work on one or two lines, always
//! instant, and predictable in a way tree-derived indentation is not when the tree is a version
//! behind. Tree-sitter indent *queries* remain the better answer for pathological cases; they belong
//! behind a synchronous parse of the current line, which is a later, separate decision.

use vex_text::{CharOffset, Text};

/// Delimiters the editor pairs, and how it indents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairConfig {
    /// Open/close pairs, in the order they are matched.
    pub brackets: Vec<(char, char)>,
    /// Symmetric delimiters, where open and close are the same character.
    pub quotes: Vec<char>,
    pub tab_width: usize,
    /// Characters that open a block when they end a line — `{`, and `:` for Python-like languages.
    pub block_openers: Vec<char>,
}

impl Default for PairConfig {
    fn default() -> Self {
        Self {
            brackets: vec![('(', ')'), ('[', ']'), ('{', '}')],
            quotes: vec!['"', '\'', '`'],
            tab_width: 4,
            // `:` is included for Python and YAML. In a C-family language a line ending in `:` is a
            // label or a match arm, where an extra level is also what you want.
            block_openers: vec!['{', '(', '[', ':'],
        }
    }
}

impl PairConfig {
    pub fn closer_for(&self, open: char) -> Option<char> {
        self.brackets
            .iter()
            .find(|(o, _)| *o == open)
            .map(|(_, c)| *c)
    }

    pub fn is_closer(&self, c: char) -> bool {
        self.brackets.iter().any(|(_, close)| *close == c)
    }

    pub fn is_quote(&self, c: char) -> bool {
        self.quotes.contains(&c)
    }
}

/// What typing one character should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeAction {
    /// Insert exactly what was typed.
    Literal,
    /// Insert an open/close pair and leave the cursor between them.
    Pair { open: char, close: char },
    /// The character typed is already there: step over it instead of inserting a duplicate.
    StepOver,
}

/// Decides what typing `typed` at `caret` should do.
///
/// The rules are conservative on purpose. Auto-pairing that fires when you did not want it is worse
/// than not having it, because the correction costs more keystrokes than the feature saved.
pub fn on_type(text: &Text, caret: CharOffset, typed: char, config: &PairConfig) -> TypeAction {
    let next = char_at(text, caret);
    let previous = caret
        .get()
        .checked_sub(1)
        .map(|index| text.rope().char(index));

    // Typing a closer that is already the next character steps over it, so `)` after autopairing
    // does the obvious thing rather than producing `))`.
    if (config.is_closer(typed) || config.is_quote(typed)) && next == Some(typed) {
        return TypeAction::StepOver;
    }

    if let Some(close) = config.closer_for(typed) {
        // Pair only when what follows is whitespace, end of line, or another closer. Typing `(`
        // before an identifier usually means wrapping it, not creating an empty call.
        return if next.is_none_or(|c| c.is_whitespace() || config.is_closer(c)) {
            TypeAction::Pair { open: typed, close }
        } else {
            TypeAction::Literal
        };
    }

    if config.is_quote(typed) {
        // Never pair a quote mid-word: `don't` must not become `don''t`, and neither must a
        // lifetime in Rust (`'a`) or a char literal.
        let after_word = previous.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let before_word = next.is_some_and(|c| c.is_alphanumeric() || c == '_');
        return if after_word || before_word {
            TypeAction::Literal
        } else {
            TypeAction::Pair {
                open: typed,
                close: typed,
            }
        };
    }

    TypeAction::Literal
}

/// Whether backspace at `caret` sits between an empty pair, so both halves should go.
pub fn deletes_pair(text: &Text, caret: CharOffset, config: &PairConfig) -> bool {
    let Some(before) = caret
        .get()
        .checked_sub(1)
        .map(|index| text.rope().char(index))
    else {
        return false;
    };
    let Some(after) = char_at(text, caret) else {
        return false;
    };

    config.closer_for(before) == Some(after) || (config.is_quote(before) && before == after)
}

/// What a newline at `caret` should produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewlineAction {
    /// Indentation for the line being opened.
    pub indent: String,
    /// When the cursor sits between an open and its close, the closer moves to its own line at the
    /// enclosing indentation and the cursor lands on the blank line between.
    pub closing_line: Option<String>,
}

/// Computes indentation for a newline at `caret`.
pub fn on_newline(text: &Text, caret: CharOffset, config: &PairConfig) -> NewlineAction {
    let point = text.char_to_point(caret);
    let line_start = text.line_to_char(point.line);

    // Only the text before the cursor matters: pressing Enter mid-line indents by what precedes it.
    let before = text.slice_to_string(line_start..caret);
    let base: String = before
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();

    let unit = " ".repeat(config.tab_width.max(1));
    let last_meaningful = before.trim_end().chars().last();
    let opens_block = last_meaningful.is_some_and(|c| config.block_openers.contains(&c));

    // Between `{` and `}`: the closer gets its own line and the cursor lands between.
    let between_pair = match (last_meaningful, char_at(text, caret)) {
        (Some(open), Some(next)) => config.closer_for(open) == Some(next),
        _ => false,
    };

    if between_pair {
        return NewlineAction {
            indent: format!("{base}{unit}"),
            closing_line: Some(base),
        };
    }

    // A line that is only a closer should not push the next line further right.
    let after = text.slice_to_string(caret..text.line_range(point.line).end);
    let closes_immediately = after
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| config.is_closer(c));

    let indent = if opens_block && !closes_immediately {
        format!("{base}{unit}")
    } else {
        base
    };

    NewlineAction {
        indent,
        closing_line: None,
    }
}

/// The indentation a line *should* have when it begins with a closing delimiter.
///
/// Used for electric dedent: typing `}` on an otherwise blank line pulls it back to match its
/// opener, which is the one indentation correction people expect an editor to make for them.
pub fn dedent_for_closer(
    text: &Text,
    caret: CharOffset,
    typed: char,
    config: &PairConfig,
) -> Option<String> {
    if !config.is_closer(typed) {
        return None;
    }

    let point = text.char_to_point(caret);
    let line_start = text.line_to_char(point.line);
    let before = text.slice_to_string(line_start..caret);

    // Only when the closer is the first thing on the line: reindenting a line with code on it would
    // move text the user placed deliberately.
    if !before.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }

    let open = config
        .brackets
        .iter()
        .find(|(_, close)| *close == typed)
        .map(|(open, _)| *open)?;

    // Walk back for the unmatched opener and copy its line's indentation.
    let mut depth = 0isize;
    let mut index = line_start.get();
    while index > 0 {
        index -= 1;
        let c = text.rope().char(index);
        if c == typed {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                let opener_line = text.char_to_line(CharOffset(index));
                let opener_start = text.line_to_char(opener_line);
                let indent: String = text
                    .slice_to_string(opener_start..text.line_range(opener_line).end)
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                return (indent != before).then_some(indent);
            }
            depth -= 1;
        }
    }

    None
}

fn char_at(text: &Text, offset: CharOffset) -> Option<char> {
    (offset.get() < text.len_chars().get()).then(|| text.rope().char(offset.get()))
}

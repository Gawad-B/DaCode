//! The prompt line: `/` search and `:` commands.
//!
//! Modelled on Vim's, because that is what the muscle memory expects: a single line at the bottom
//! that takes over the statusline while active. It is not another editor mode — the modal grammar
//! stays out of it, exactly as the palette does — but unlike the palette it is a line, not a card,
//! because `/` is used mid-thought and a large overlay would hide the text being searched.

use dacode_text::CharOffset;
use std::ops::Range;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// `/` and `?`
    Search { forward: bool },
    /// `:`
    Command,
    /// A folder to open as the project, pre-filled with the current root.
    OpenFolder,
    /// A colour-scheme file to read, in any format the importer understands.
    LoadTheme,
    /// A rename, pre-filled with the symbol under the cursor.
    ///
    /// A prompt rather than a modal dialog, and pre-filled rather than empty, because a rename is
    /// almost always an edit of the existing name — retyping it from scratch is how you rename
    /// `value` to `vlaue`.
    Rename,
    /// A path to write this buffer to, pre-filled with where it already is.
    ///
    /// Pre-filled for the same reason a rename is: "save as" is nearly always a small edit of the
    /// current name — a copy, a different extension — and retyping the directory is how you save it
    /// somewhere you did not mean to.
    SaveAs,
    /// A name for a file to create, pre-filled with the directory it will go in.
    NewFile,
    /// A new name for a file on disk, pre-filled with the current one.
    ///
    /// Pre-filled for the same reason a symbol rename is: it is nearly always a small edit of what
    /// is there, and retyping the path is how you move a file somewhere you did not mean to.
    RenameFile,
    /// `ctrl-h` — find and replace, in this file.
    ///
    /// Two fields on one line rather than an ex-command: `:s/a/b/g` is Vim-shaped, and with the
    /// grammar off no surface may teach it. It reports itself as a search, so the live highlighting,
    /// the match counter and the step-to-next keys are the ones `/` and `ctrl-f` already have —
    /// replace is search with somewhere to put the answer, not a second feature.
    Replace,
}

/// Which half of a two-field prompt has the caret.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Field {
    Query,
    Replacement,
}

impl PromptKind {
    /// The character shown before the input, as Vim does.
    pub fn sigil(self) -> &'static str {
        match self {
            PromptKind::Search { forward: true } => "/",
            PromptKind::Search { forward: false } => "?",
            PromptKind::Command => ":",
            PromptKind::Rename => "rename to ",
            PromptKind::OpenFolder => "open folder ",
            PromptKind::LoadTheme => "theme file ",
            PromptKind::SaveAs => "save as ",
            PromptKind::Replace => "replace ",
            PromptKind::NewFile => "new file ",
            PromptKind::RenameFile => "rename file to ",
        }
    }
}

#[derive(Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub input: String,
    /// Where the cursor was when the prompt opened.
    ///
    /// Incremental search moves the cursor as you type; cancelling has to put it back, or `/` becomes
    /// a way to lose your place.
    pub origin: CharOffset,
    /// Matches for the current query, when this is a search.
    pub matches: Vec<Range<CharOffset>>,
    /// A message to show instead of the input — an error from a command that failed.
    pub message: Option<String>,
    /// Whether the user has stepped through matches with `↵`.
    ///
    /// It decides what escape means. Cancelling a search you only typed should put you back where
    /// you were — that is why `origin` exists. But once you have deliberately walked to the fourth
    /// match, being thrown back to where you started is not a cancel, it is losing your place, which
    /// is the exact thing `origin` was added to prevent.
    pub stepped: bool,
    /// What to put in place of each match. Only a replace prompt has one.
    pub replacement: String,
    /// Which field the caret is in. `tab` moves it.
    pub focus: Field,
}

impl Prompt {
    pub fn new(kind: PromptKind, origin: CharOffset) -> Self {
        Self {
            kind,
            input: String::new(),
            origin,
            matches: Vec::new(),
            message: None,
            stepped: false,
            replacement: String::new(),
            focus: Field::Query,
        }
    }

    /// Whether this prompt searches as you type.
    ///
    /// Replace says yes: it wants the same live highlighting, the same match counter and the same
    /// step-to-next keys, and every caller that asks this question wants all three.
    pub fn is_search(&self) -> bool {
        matches!(self.kind, PromptKind::Search { .. } | PromptKind::Replace)
    }

    pub fn is_replace(&self) -> bool {
        self.kind == PromptKind::Replace
    }

    pub fn forward(&self) -> bool {
        match self.kind {
            PromptKind::Search { forward } => forward,
            PromptKind::Command
            | PromptKind::Rename
            | PromptKind::OpenFolder
            | PromptKind::LoadTheme
            | PromptKind::SaveAs
            | PromptKind::NewFile
            | PromptKind::RenameFile
            | PromptKind::Replace => true,
        }
    }

    /// The field the caret is in, for typing into.
    pub fn field_mut(&mut self) -> &mut String {
        match (self.kind, self.focus) {
            (PromptKind::Replace, Field::Replacement) => &mut self.replacement,
            _ => &mut self.input,
        }
    }

    /// Moves between the two fields. A no-op for prompts with one.
    pub fn toggle_field(&mut self) {
        if self.is_replace() {
            self.focus = match self.focus {
                Field::Query => Field::Replacement,
                Field::Replacement => Field::Query,
            };
        }
    }

    /// What the statusline shows while the prompt is open.
    pub fn display(&self) -> String {
        self.display_with_caret().0
    }

    /// The line, and how many characters into it the caret sits.
    ///
    /// One function for both, because the caret is drawn by counting characters into the displayed
    /// string. Computing the string in one place and the caret index in another is the same
    /// duplication `layout.rs` exists to prevent, one line further down the window.
    pub fn display_with_caret(&self) -> (String, usize) {
        if let Some(message) = &self.message {
            return (message.clone(), message.chars().count());
        }

        if !self.is_replace() {
            let line = format!("{}{}", self.kind.sigil(), self.input);
            let caret = line.chars().count();
            return (line, caret);
        }

        // `replace needle with something` — words rather than `s/needle/something/`, which is Vim's
        // spelling of it and is exactly what the non-modal editor must never show.
        let head = format!("{}{}", self.kind.sigil(), self.input);
        let caret = match self.focus {
            Field::Query => head.chars().count(),
            Field::Replacement => {
                head.chars().count() + " with ".chars().count() + self.replacement.chars().count()
            }
        };
        (format!("{head} with {}", self.replacement), caret)
    }
}

/// A parsed `:` command.
///
/// Only commands with an implementation are recognized. An unknown command reports itself rather
/// than doing nothing, so a typo is distinguishable from an unimplemented feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Write {
        path: Option<String>,
    },
    Quit {
        force: bool,
    },
    WriteQuit,
    /// `:42` — jump to a line, 1-based as everything the user sees.
    GotoLine(usize),
    Edit {
        path: String,
    },
    /// `:grep pattern` — project search.
    Grep {
        pattern: String,
    },
    /// `:format` — ask the language server to reformat the buffer.
    Format,
    /// `:rename [name]` — rename the symbol under the cursor. Without a name, opens the prompt.
    Rename {
        new_name: Option<String>,
    },
    /// `:replace old new` — replace across the files the current search found.
    Replace {
        pattern: String,
        replacement: String,
    },
    SetOption {
        name: String,
        value: bool,
    },
    Unknown(String),
}

pub fn parse_command(input: &str) -> Command {
    let input = input.trim();

    // A bare number is a line jump, which is why it is checked before any name lookup.
    if let Ok(line) = input.parse::<usize>() {
        return Command::GotoLine(line);
    }

    let (name, rest) = match input.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (input, ""),
    };

    match name {
        "w" | "write" => Command::Write {
            path: (!rest.is_empty()).then(|| rest.to_string()),
        },
        "q" | "quit" => Command::Quit { force: false },
        "q!" | "quit!" => Command::Quit { force: true },
        "wq" | "x" => Command::WriteQuit,
        "e" | "edit" => Command::Edit {
            path: rest.to_string(),
        },
        "grep" | "search" => Command::Grep {
            pattern: rest.to_string(),
        },
        "fmt" | "format" => Command::Format,
        // Two arguments, split on whitespace. Not Vim's `:%s/old/new/` — that syntax needs escaping
        // rules for the delimiter, and this operates on search results rather than on a range.
        "replace" => {
            let (pattern, replacement) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            Command::Replace {
                pattern: pattern.to_string(),
                replacement: replacement.trim().to_string(),
            }
        }
        "rename" => Command::Rename {
            new_name: (!rest.is_empty()).then(|| rest.to_string()),
        },
        "set" => {
            // `:set wrap` / `:set nowrap`, the Vim spelling.
            let (value, option) = match rest.strip_prefix("no") {
                Some(option) => (false, option),
                None => (true, rest),
            };
            Command::SetOption {
                name: option.to_string(),
                value,
            }
        }
        other => Command::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_number_is_a_line_jump() {
        assert_eq!(parse_command("42"), Command::GotoLine(42));
        assert_eq!(parse_command(" 7 "), Command::GotoLine(7));
    }

    #[test]
    fn write_and_quit_spellings_all_work() {
        assert_eq!(parse_command("w"), Command::Write { path: None });
        assert_eq!(parse_command("write"), Command::Write { path: None });
        assert_eq!(
            parse_command("w out.rs"),
            Command::Write {
                path: Some("out.rs".into())
            }
        );
        assert_eq!(parse_command("q"), Command::Quit { force: false });
        assert_eq!(parse_command("q!"), Command::Quit { force: true });
        assert_eq!(parse_command("wq"), Command::WriteQuit);
        assert_eq!(parse_command("x"), Command::WriteQuit);
    }

    #[test]
    fn set_understands_the_no_prefix() {
        assert_eq!(
            parse_command("set wrap"),
            Command::SetOption {
                name: "wrap".into(),
                value: true
            }
        );
        assert_eq!(
            parse_command("set nowrap"),
            Command::SetOption {
                name: "wrap".into(),
                value: false
            }
        );
    }

    #[test]
    fn grep_keeps_the_whole_pattern_including_spaces() {
        assert_eq!(
            parse_command("grep fn main() {"),
            Command::Grep {
                pattern: "fn main() {".into()
            }
        );
    }

    #[test]
    fn an_unknown_command_names_itself() {
        // A typo must be distinguishable from an unimplemented feature.
        assert_eq!(parse_command("wrtie"), Command::Unknown("wrtie".into()));
    }

    #[test]
    fn the_sigil_matches_the_direction() {
        assert_eq!(PromptKind::Search { forward: true }.sigil(), "/");
        assert_eq!(PromptKind::Search { forward: false }.sigil(), "?");
        assert_eq!(PromptKind::Command.sigil(), ":");
    }

    #[test]
    fn the_display_shows_the_sigil_then_the_input() {
        let mut prompt = Prompt::new(PromptKind::Search { forward: true }, CharOffset(0));
        prompt.input.push_str("needle");
        assert_eq!(prompt.display(), "/needle");

        // A message replaces it, so an error is not appended to a query the user must then clear.
        prompt.message = Some("not found: needle".into());
        assert_eq!(prompt.display(), "not found: needle");
    }

    #[test]
    fn replace_takes_two_arguments() {
        assert_eq!(
            parse_command("replace old new"),
            Command::Replace {
                pattern: "old".into(),
                replacement: "new".into()
            }
        );
        // Deleting every occurrence is a replacement with nothing, which has to be expressible.
        assert_eq!(
            parse_command("replace old"),
            Command::Replace {
                pattern: "old".into(),
                replacement: String::new()
            }
        );
    }

    #[test]
    fn format_and_rename_parse() {
        assert_eq!(parse_command("format"), Command::Format);
        assert_eq!(parse_command("fmt"), Command::Format);
        assert_eq!(
            parse_command("rename total"),
            Command::Rename {
                new_name: Some("total".into())
            }
        );
        assert_eq!(parse_command("rename"), Command::Rename { new_name: None });
    }
}

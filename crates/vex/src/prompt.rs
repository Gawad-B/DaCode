//! The prompt line: `/` search and `:` commands.
//!
//! Modelled on Vim's, because that is what the muscle memory expects: a single line at the bottom
//! that takes over the statusline while active. It is not another editor mode — the modal grammar
//! stays out of it, exactly as the palette does — but unlike the palette it is a line, not a card,
//! because `/` is used mid-thought and a large overlay would hide the text being searched.

use std::ops::Range;
use vex_text::CharOffset;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// `/` and `?`
    Search { forward: bool },
    /// `:`
    Command,
}

impl PromptKind {
    /// The character shown before the input, as Vim does.
    pub fn sigil(self) -> &'static str {
        match self {
            PromptKind::Search { forward: true } => "/",
            PromptKind::Search { forward: false } => "?",
            PromptKind::Command => ":",
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
}

impl Prompt {
    pub fn new(kind: PromptKind, origin: CharOffset) -> Self {
        Self {
            kind,
            input: String::new(),
            origin,
            matches: Vec::new(),
            message: None,
        }
    }

    pub fn is_search(&self) -> bool {
        matches!(self.kind, PromptKind::Search { .. })
    }

    pub fn forward(&self) -> bool {
        match self.kind {
            PromptKind::Search { forward } => forward,
            PromptKind::Command => true,
        }
    }

    /// What the statusline shows while the prompt is open.
    pub fn display(&self) -> String {
        match &self.message {
            Some(message) => message.clone(),
            None => format!("{}{}", self.kind.sigil(), self.input),
        }
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
}

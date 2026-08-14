//! The help panel: which keys do what, right now, without leaving the editor.
//!
//! `CHEATSHEET.md` has always had this, and being a file on disk it has always been read by nobody
//! who needed it — a person who cannot work out how to leave insert mode is not going to find a
//! markdown file in a repository. F1 is where every application in the world keeps this.
//!
//! **It shows the keys for the grammar you are actually using.** A non-modal user reading `dd` and
//! `ciw` learns nothing except that this editor is not for them, which was the exact complaint that
//! prompted all of this. The same table drives both, filtered by mode — see `vex_input::commands`.
//!
//! Filterable, because a list of seventy keys is a reference and not an answer: typing "comment"
//! leaves the one line you wanted.

use vex_input::commands::{self, Command, Group};

/// The panel's state.
#[derive(Debug, Clone)]
pub struct Help {
    /// Which grammar's keys are being shown.
    pub modal: bool,
    /// What has been typed to narrow the list.
    pub query: String,
    /// First visible row, for scrolling.
    pub scroll: usize,
}

/// A line in the panel: either a heading or a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Heading(&'static str),
    Binding {
        keys: &'static str,
        what: &'static str,
    },
}

impl Help {
    pub fn new(modal: bool) -> Self {
        Self {
            modal,
            query: String::new(),
            scroll: 0,
        }
    }

    /// The lines to draw.
    ///
    /// Commands with no key in this mode are left out. The panel answers "what can I press", and a
    /// row with an empty key column answers something else — the palette is where those live.
    pub fn lines(&self) -> Vec<Line> {
        let query = self.query.trim().to_lowercase();
        let mut lines = Vec::new();

        for group in Group::ALL {
            let matching: Vec<&Command> = commands::in_group(group)
                .filter(|command| !command.keys_for(self.modal).is_empty())
                .filter(|command| matches(command, &query, self.modal))
                .collect();

            // A heading with nothing under it is noise, and eight of them is a panel that looks
            // full while saying nothing.
            if matching.is_empty() {
                continue;
            }
            lines.push(Line::Heading(group.title()));
            for command in matching {
                lines.push(Line::Binding {
                    keys: command.keys_for(self.modal),
                    what: command.what,
                });
            }
        }

        lines
    }

    /// Scrolls, clamped to the end of the list.
    pub fn scroll_by(&mut self, delta: isize, rows: usize) {
        let total = self.lines().len();
        let last = total.saturating_sub(rows);
        self.scroll = match delta {
            d if d < 0 => self.scroll.saturating_sub(d.unsigned_abs()),
            d => (self.scroll + d as usize).min(last),
        };
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.scroll = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.scroll = 0;
    }
}

fn matches(command: &Command, query: &str, modal: bool) -> bool {
    if query.is_empty() {
        return true;
    }
    // The keys are searchable too, which is the other direction people use this panel in: "what
    // does ctrl-d do again".
    command.what.to_lowercase().contains(query)
        || command.name.contains(query)
        || command.keys_for(modal).contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_shows_the_keys_for_the_grammar_in_use() {
        // The complaint this whole change came from: turning Vim off changed almost nothing visible.
        // The help panel is where that difference has to be most obvious, so a non-modal user must
        // not be shown `dd` and a modal one must not be shown `<c-s-k>`.
        let flat = Help::new(false).lines();
        let modal = Help::new(true).lines();

        assert!(flat.contains(&Line::Binding {
            keys: "<c-/>",
            what: "comment or uncomment these lines"
        }));
        assert!(
            !flat
                .iter()
                .any(|line| matches!(line, Line::Binding { keys, .. } if *keys == "dd")),
            "a non-modal user has no dd"
        );
        assert!(
            modal
                .iter()
                .any(|line| matches!(line, Line::Binding { keys, .. } if keys.contains("dd"))),
            "a modal user does"
        );
    }

    #[test]
    fn every_heading_has_something_under_it() {
        for modal in [true, false] {
            let lines = Help::new(modal).lines();
            for (index, line) in lines.iter().enumerate() {
                if matches!(line, Line::Heading(_)) {
                    assert!(
                        matches!(lines.get(index + 1), Some(Line::Binding { .. })),
                        "an empty heading in the {} list",
                        if modal { "modal" } else { "flat" }
                    );
                }
            }
        }
    }

    #[test]
    fn a_query_narrows_to_what_was_asked_for() {
        let mut help = Help::new(false);
        for c in "comment".chars() {
            help.type_char(c);
        }
        let lines = help.lines();
        assert!(
            lines.iter().all(
                |line| !matches!(line, Line::Binding { what, .. } if !what.contains("comment"))
            ),
            "the filter left something unrelated behind"
        );
        assert!(lines.len() >= 2, "and left the heading and the answer");
    }

    #[test]
    fn a_key_can_be_looked_up_by_what_you_pressed() {
        // The other way people use this: they pressed something by accident and want to know what
        // it was.
        let mut help = Help::new(false);
        for c in "<c-d>".chars() {
            help.type_char(c);
        }
        assert!(
            help.lines().iter().any(|line| {
                matches!(line, Line::Binding { what, .. } if what.contains("next copy"))
            }),
            "searching for a key should find what it does"
        );
    }

    #[test]
    fn scrolling_stops_at_the_end() {
        let mut help = Help::new(true);
        help.scroll_by(1000, 10);
        let total = help.lines().len();
        assert_eq!(help.scroll, total.saturating_sub(10));
        help.scroll_by(-1000, 10);
        assert_eq!(help.scroll, 0);
    }
}

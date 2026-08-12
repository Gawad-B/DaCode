//! The completion popup.
//!
//! Completion is the LSP feature with the tightest latency budget: it is requested *while* typing, and
//! a suggestion that arrives after you have typed past it is worse than none — it either flashes and
//! vanishes or, worse, inserts something stale. So the rules here are about staleness and about
//! filtering locally rather than round-tripping the server for every keystroke.
//!
//! The server is asked once per completion *session*; each subsequent character filters the list the
//! editor already has. That is how the popup keeps up with typing without asking a language server to
//! be fast.

use vex_text::{CharOffset, Text, Version};

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// Character positions in `label` matched by the current prefix, for highlighting.
    pub highlights: Vec<u32>,
}

/// Rows shown at once. A popup taller than this stops being a glance and becomes a list to read.
pub const VISIBLE_ROWS: usize = 8;

pub struct Completion {
    /// Where the word being completed starts — what gets replaced on accept.
    pub anchor: CharOffset,
    /// Everything the server offered, unfiltered.
    all: Vec<String>,
    /// What survives the current prefix.
    pub items: Vec<Item>,
    pub selected: usize,
    /// The document version the request was made against.
    ///
    /// The popup closes if the buffer moves on in a way that invalidates the anchor, rather than
    /// inserting text computed for a document that no longer exists.
    pub version: Version,
    /// Set until the reply arrives, so the UI can say it is waiting instead of showing nothing.
    pub pending: bool,
}

impl Completion {
    /// Opens a session for the word ending at `caret`.
    pub fn new(text: &Text, caret: CharOffset, version: Version) -> Self {
        Self {
            anchor: word_start(text, caret),
            all: Vec::new(),
            items: Vec::new(),
            selected: 0,
            version,
            pending: true,
        }
    }

    /// Accepts the server's list and applies the current prefix to it.
    pub fn offer(&mut self, labels: Vec<String>, text: &Text, caret: CharOffset) {
        self.all = labels;
        self.pending = false;
        self.refilter(text, caret);
    }

    /// The text typed since the session opened.
    pub fn prefix(&self, text: &Text, caret: CharOffset) -> String {
        if caret <= self.anchor {
            return String::new();
        }
        text.slice_to_string(self.anchor..caret)
    }

    /// Narrows the list to what still matches. Called on every keystroke; no server involved.
    pub fn refilter(&mut self, text: &Text, caret: CharOffset) {
        let prefix = self.prefix(text, caret);
        let lowered = prefix.to_lowercase();

        self.items = self
            .all
            .iter()
            .filter_map(|label| {
                if prefix.is_empty() {
                    return Some(Item {
                        label: label.clone(),
                        highlights: Vec::new(),
                    });
                }
                // Prefix match, case-insensitively. Fuzzy matching belongs in the palette, where you
                // are searching; here you are typing an identifier and expect it to narrow the way a
                // shell completes a path.
                let matches = label.to_lowercase().starts_with(&lowered);
                matches.then(|| Item {
                    label: label.clone(),
                    highlights: (0..prefix.chars().count() as u32).collect(),
                })
            })
            .take(VISIBLE_ROWS * 6)
            .collect();

        // Shortest first: with a prefix, the shortest completion is usually the one meant, and it is
        // the one a second keystroke would disambiguate away from anyway.
        self.items.sort_by(|a, b| {
            a.label
                .len()
                .cmp(&b.label.len())
                .then_with(|| a.label.cmp(&b.label))
        });
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    /// Whether the session is still valid for this cursor and document version.
    ///
    /// A session dies when the cursor moves before its anchor (you backspaced out of the word) or the
    /// document changed in a way the anchor cannot survive. Keeping it alive past that is how editors
    /// end up inserting a suggestion for text you already deleted.
    pub fn is_valid(&self, caret: CharOffset) -> bool {
        caret >= self.anchor
    }

    pub fn selection(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        self.selected = match delta {
            // Wrapping, because a popup is short and cycling beats hitting a wall.
            d if d < 0 => self.selected.checked_sub(d.unsigned_abs()).unwrap_or(last),
            d => {
                let next = self.selected + d as usize;
                if next > last { 0 } else { next }
            }
        };
    }

    /// The window of items to draw, and the selection's index within it.
    pub fn visible(&self) -> (&[Item], usize) {
        if self.items.len() <= VISIBLE_ROWS {
            return (&self.items, self.selected);
        }
        let start = self
            .selected
            .saturating_sub(VISIBLE_ROWS - 1)
            .min(self.items.len() - VISIBLE_ROWS);
        (
            &self.items[start..start + VISIBLE_ROWS],
            self.selected - start,
        )
    }
}

/// The start of the identifier ending at `caret`.
///
/// `_` counts as part of a word and `.`/`:` do not, so `foo.ba|` completes `ba` rather than `foo.ba`.
fn word_start(text: &Text, caret: CharOffset) -> CharOffset {
    let mut start = caret.get();
    while start > 0 {
        let c = text.rope().char(start - 1);
        if c.is_alphanumeric() || c == '_' {
            start -= 1;
        } else {
            break;
        }
    }
    CharOffset(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(source: &str) -> Text {
        Text::from_str(source)
    }

    #[test]
    fn the_session_anchors_at_the_start_of_the_word() {
        let text = text("let value = foo.ba");
        let caret = text.len_chars();
        let completion = Completion::new(&text, caret, Version(1));

        assert_eq!(
            text.slice_to_string(completion.anchor..caret),
            "ba",
            "the anchor is the identifier, not the whole expression"
        );
    }

    #[test]
    fn an_anchor_at_a_boundary_is_an_empty_prefix() {
        let text = text("foo.");
        let caret = text.len_chars();
        let completion = Completion::new(&text, caret, Version(1));
        assert_eq!(completion.prefix(&text, caret), "");
    }

    #[test]
    fn typing_narrows_the_list_without_asking_the_server_again() {
        let source = text("le");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(
            vec!["len".into(), "length".into(), "let".into(), "clone".into()],
            &source,
            caret,
        );

        // `le` matches three of the four.
        assert_eq!(completion.items.len(), 3);
        assert!(!completion.pending);

        // Another character narrows locally: the same list, filtered.
        let longer = text("len");
        let caret = longer.len_chars();
        completion.refilter(&longer, caret);
        assert_eq!(
            completion
                .items
                .iter()
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>(),
            vec!["len", "length"]
        );
    }

    #[test]
    fn shortest_match_sorts_first() {
        let source = text("le");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(
            vec!["length_of_thing".into(), "len".into(), "letter".into()],
            &source,
            caret,
        );

        assert_eq!(completion.items[0].label, "len");
    }

    #[test]
    fn matching_is_case_insensitive_but_the_prefix_is_highlighted() {
        let source = text("va");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(vec!["Value".into(), "VARIANT".into()], &source, caret);

        assert_eq!(completion.items.len(), 2);
        assert_eq!(
            completion.items[0].highlights,
            vec![0, 1],
            "the typed prefix is what gets highlighted"
        );
    }

    #[test]
    fn a_prefix_matching_nothing_empties_the_list() {
        let source = text("zz");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(vec!["alpha".into(), "beta".into()], &source, caret);

        assert!(completion.items.is_empty());
        assert!(completion.selection().is_none());
    }

    #[test]
    fn the_session_dies_when_the_cursor_moves_before_its_anchor() {
        // Backspacing out of the word must not leave a popup that inserts a suggestion for text that
        // is no longer there. The word is preceded by other text so the anchor is not offset zero —
        // with `value` alone the anchor *is* zero and nothing can be before it.
        let source = text("let value");
        let caret = source.len_chars();
        let completion = Completion::new(&source, caret, Version(1));

        assert_eq!(completion.anchor, CharOffset(4), "anchored at `value`");
        assert!(completion.is_valid(caret));
        assert!(
            !completion.is_valid(CharOffset(2)),
            "a cursor before the anchor ends the session"
        );
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let source = text("a");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(vec!["ab".into(), "ac".into(), "ad".into()], &source, caret);

        completion.move_selection(1);
        assert_eq!(completion.selected, 1);
        completion.move_selection(5);
        assert_eq!(completion.selected, 0, "past the end wraps to the start");
        completion.move_selection(-1);
        assert_eq!(completion.selected, 2, "before the start wraps to the end");
    }

    #[test]
    fn the_window_keeps_the_selection_visible() {
        let source = text("a");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        let labels: Vec<String> = (0..30).map(|i| format!("a_item_{i:02}")).collect();
        completion.offer(labels, &source, caret);

        completion.move_selection(VISIBLE_ROWS as isize + 1);
        let (rows, cursor) = completion.visible();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert!(cursor < VISIBLE_ROWS);
        assert_eq!(
            rows[cursor].label,
            completion.items[completion.selected].label
        );
    }

    #[test]
    fn a_pending_session_has_no_items_yet() {
        let source = text("le");
        let completion = Completion::new(&source, source.len_chars(), Version(1));
        assert!(completion.pending);
        assert!(completion.items.is_empty());
    }
}

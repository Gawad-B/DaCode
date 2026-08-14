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

/// One suggestion, filtered in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Everything the server said about it, kept whole — the popup shows the type, and accepting
    /// needs the insert text and the import edits.
    pub offer: vex_lsp::CompletionItem,
    /// Where it sits in the unfiltered list, which is what a resolve reply names.
    pub at: usize,
    /// Character positions in the label matched by the current prefix, for highlighting.
    pub highlights: Vec<u32>,
}

impl Item {
    pub fn label(&self) -> &str {
        &self.offer.label
    }
}

/// Rows shown at once. A popup taller than this stops being a glance and becomes a list to read.
pub const VISIBLE_ROWS: usize = 8;

pub struct Completion {
    /// Where the word being completed starts — what gets replaced on accept.
    pub anchor: CharOffset,
    /// Everything the server offered, unfiltered.
    all: Vec<vex_lsp::CompletionItem>,
    /// What survives the current prefix.
    pub items: Vec<Item>,
    pub selected: usize,
    /// The document version the request was made against.
    ///
    /// The popup closes if the buffer moves on in a way that invalidates the anchor, rather than
    /// inserting text computed for a document that no longer exists.
    pub version: Version,
    /// How many lines the document had when the request went out.
    ///
    /// The server's `additionalTextEdits` are line-and-column, so typing inside the word being
    /// completed cannot move them — but adding or removing a *line* can, and then an import would
    /// land in the middle of some other statement. Cheaper to notice than to reason about.
    lines: usize,
    /// Set until the reply arrives, so the UI can say it is waiting instead of showing nothing.
    pub pending: bool,
    /// The server said its list was truncated and it wants asking again as more is typed.
    pub incomplete: bool,
    /// Which items have already been sent for resolving, so none is asked for twice.
    asked: std::collections::HashSet<usize>,
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
            lines: text.last_line() + 1,
            pending: true,
            incomplete: false,
            asked: std::collections::HashSet::new(),
        }
    }

    /// Accepts the server's list and applies the current prefix to it.
    pub fn offer(
        &mut self,
        items: Vec<vex_lsp::CompletionItem>,
        incomplete: bool,
        text: &Text,
        caret: CharOffset,
    ) {
        self.all = items;
        self.incomplete = incomplete;
        self.pending = false;
        // A fresh list means fresh indices; anything asked for against the old one is meaningless.
        self.asked.clear();
        self.refilter(text, caret);
    }

    /// The suggestion whose rest is worth asking for, if there is one.
    ///
    /// **Only the selected one, and only once.** A server is entitled to do real work per resolve —
    /// rust-analyzer computes the import path — so resolving a hundred candidates because they are
    /// on screen would be asking it to do a hundred times the work for the one answer that gets
    /// used. Asking as the selection moves means the item is nearly always filled in before it is
    /// accepted, and accepting one that is not simply goes without its import and says so.
    pub fn wants_resolving(&mut self) -> Option<(usize, serde_json::Value)> {
        let at = self.items.get(self.selected)?.at;
        if !self.asked.insert(at) {
            return None;
        }
        Some((at, self.all.get(at)?.raw.clone()))
    }

    /// Merges a resolved suggestion back into the list.
    pub fn resolved(&mut self, at: usize, item: vex_lsp::CompletionItem) {
        let Some(existing) = self.all.get_mut(at) else {
            return;
        };
        // The reply is allowed to leave fields out, which means "unchanged" rather than "empty" —
        // a resolve that blanked the label would erase the row the user is looking at.
        if !item.additional.is_empty() {
            existing.additional = item.additional;
        }
        if item.detail.is_some() {
            existing.detail = item.detail;
        }
        // Whatever is on screen came from `all`, so it has to be rebuilt to show any of this.
        for shown in &mut self.items {
            if shown.at == at {
                shown.offer = existing.clone();
            }
        }
    }

    /// Whether edits the server placed elsewhere in the file can still be trusted.
    ///
    /// They are line-and-column, and an edit *within* one line never moves another line's
    /// coordinates — which is the only kind of edit a live completion session can have seen, since
    /// it dies the moment the caret leaves its word. A changed line count means something else
    /// happened, and an import inserted at a guessed line is worse than no import.
    pub fn imports_are_current(&self, text: &Text) -> bool {
        text.last_line() + 1 == self.lines
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
            .enumerate()
            .filter_map(|(at, offer)| {
                if prefix.is_empty() {
                    return Some(Item {
                        offer: offer.clone(),
                        at,
                        highlights: Vec::new(),
                    });
                }
                // Prefix match, case-insensitively, against `filterText` when the server sent one:
                // rust-analyzer labels a method `foo(…)` and filters it as `foo`, so matching the
                // label would lose it the moment you typed the second character.
                let against = offer.filter.as_deref().unwrap_or(&offer.label);
                let matches = against.to_lowercase().starts_with(&lowered);
                matches.then(|| Item {
                    offer: offer.clone(),
                    at,
                    // The highlight is on the *label*, which may be longer than what was filtered
                    // against — never highlight past its end.
                    highlights: (0..prefix.chars().count().min(offer.label.chars().count()) as u32)
                        .collect(),
                })
            })
            .take(VISIBLE_ROWS * 6)
            .collect();

        // The server's own ranking when it gave one. rust-analyzer's `sortText` encodes what is in
        // scope, what the expected type is, and how recently something was used — none of which an
        // editor can recover from a list of strings, and all of which the old shortest-first rule
        // actively destroyed by re-sorting on top of it.
        //
        // Without a `sortText`, shortest first: with a prefix typed, the shortest completion is
        // usually the one meant, and it is the one another keystroke would disambiguate away from.
        let ranked = self.items.iter().any(|item| item.offer.sort.is_some());
        self.items.sort_by(|a, b| {
            let key = |item: &Item| {
                (
                    ranked.then(|| item.offer.sort.clone().unwrap_or_default()),
                    item.offer.label.len(),
                    item.offer.label.clone(),
                )
            };
            key(a).cmp(&key(b))
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

    /// A suggestion that says nothing but its name — what a plain server sends.
    fn offer(label: &str) -> vex_lsp::CompletionItem {
        vex_lsp::CompletionItem {
            label: label.to_string(),
            insert: label.to_string(),
            ..Default::default()
        }
    }

    fn offers(labels: &[&str]) -> Vec<vex_lsp::CompletionItem> {
        labels.iter().map(|label| offer(label)).collect()
    }

    fn labels(completion: &Completion) -> Vec<&str> {
        completion.items.iter().map(Item::label).collect()
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
            offers(&["len", "length", "let", "clone"]),
            false,
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
        assert_eq!(labels(&completion), vec!["len", "length"]);
    }

    #[test]
    fn the_servers_own_ranking_wins_where_it_gave_one() {
        // rust-analyzer's sortText encodes what is in scope, what type is expected and what was
        // used recently — none of which can be recovered from a list of strings. Re-sorting by
        // length on top of it, which is what this used to do, threw all of that away.
        let source = text("f");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));

        let ranked = |label: &str, sort: &str| vex_lsp::CompletionItem {
            label: label.into(),
            insert: label.into(),
            sort: Some(sort.into()),
            ..Default::default()
        };
        completion.offer(
            vec![
                ranked("format_args_nl", "ffffdddd"),
                ranked("from", "ffff0000"),
                ranked("fold", "ffff2222"),
            ],
            false,
            &source,
            caret,
        );

        assert_eq!(
            labels(&completion),
            vec!["from", "fold", "format_args_nl"],
            "the server's order, not the shortest first"
        );
    }

    #[test]
    fn filter_text_is_matched_when_the_label_would_not_be() {
        // rust-analyzer labels a method `foo(…)` and filters it as `foo`. Matching the label would
        // lose the suggestion the moment a second character was typed.
        let source = text("wr");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(
            vec![vex_lsp::CompletionItem {
                label: "write_all(…)".into(),
                insert: "write_all(${1:buf})".into(),
                filter: Some("write_all".into()),
                snippet: true,
                ..Default::default()
            }],
            false,
            &source,
            caret,
        );

        assert_eq!(labels(&completion), vec!["write_all(…)"]);
        assert_eq!(
            completion.items[0].highlights,
            vec![0, 1],
            "and the highlight stays inside the label"
        );
    }

    #[test]
    fn imports_are_refused_once_the_file_has_changed_shape() {
        // The server's extra edits are line-and-column. Typing inside the word cannot move another
        // line, so they stay good; adding a line can, and an import landing mid-statement is worse
        // than no import at all.
        let source = text("use std::io;\n\nfn main() { Has }\n");
        let completion = Completion::new(&source, CharOffset(29), Version(1));

        assert!(completion.imports_are_current(&source));
        assert!(
            completion.imports_are_current(&text("use std::io;\n\nfn main() { HashM }\n")),
            "typing inside the word leaves every other line where it was"
        );
        assert!(
            !completion.imports_are_current(&text("use std::io;\n\n\nfn main() { Has }\n")),
            "a new line moves them, and vex cannot know by how much"
        );
    }

    #[test]
    fn shortest_match_sorts_first() {
        let source = text("le");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(
            offers(&["length_of_thing", "len", "letter"]),
            false,
            &source,
            caret,
        );

        assert_eq!(
            completion.items[0].label(),
            "len",
            "with no ranking from the server, the shortest is the one meant"
        );
    }

    #[test]
    fn matching_is_case_insensitive_but_the_prefix_is_highlighted() {
        let source = text("va");
        let caret = source.len_chars();
        let mut completion = Completion::new(&source, caret, Version(1));
        completion.offer(offers(&["Value", "VARIANT"]), false, &source, caret);

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
        completion.offer(offers(&["alpha", "beta"]), false, &source, caret);

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
        completion.offer(offers(&["ab", "ac", "ad"]), false, &source, caret);

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
        let many: Vec<String> = (0..30).map(|i| format!("a_item_{i:02}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        completion.offer(offers(&many), false, &source, caret);

        completion.move_selection(VISIBLE_ROWS as isize + 1);
        let (rows, cursor) = completion.visible();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert!(cursor < VISIBLE_ROWS);
        assert_eq!(
            rows[cursor].label(),
            completion.items[completion.selected].label()
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

//! Expanding snippets, with tab stops.
//!
//! The importer has been able to *read* snippets since M5, and said plainly in its report that
//! expansion was pending. This is that.
//!
//! A snippet is expanded into plain text plus a list of positions, and the editor then walks those
//! positions with `tab`. Deliberately not a live template that rewrites itself as you type: mirrored
//! placeholders (`$1` appearing twice, updating together) need a second edit path that runs on every
//! keystroke, and every editor that has one has a long history of bugs where it fires inside a string
//! literal you were editing. Positions are anchors, so they survive the edits you make between them.

use dacode_text::{Anchor, Bias, CharOffset, Edit};

/// A snippet ready to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// The text to insert, with placeholder syntax removed.
    pub text: String,
    /// Where the cursor should stop, in order, as offsets into `text`.
    pub stops: Vec<usize>,
    /// For each stop, how many characters of default text it selects.
    pub lengths: Vec<usize>,
}

/// Turns a snippet body into text and stop positions.
///
/// Understands `$1`, `${1:default}` and `$0`. `$0` is the final stop wherever it appears, which is
/// what the LSP and VSCode both mean by it; a body with no `$0` gets one at the end.
pub fn expand(body: &str, indent: &str) -> Expansion {
    let mut text = String::with_capacity(body.len());
    let mut found: Vec<(u32, usize, usize)> = Vec::new();

    let chars: Vec<char> = body.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        // Continuation lines take the indentation of the line the snippet was triggered on, or a
        // multi-line snippet lands flush against the left margin.
        if chars[index] == '\n' {
            text.push('\n');
            text.push_str(indent);
            index += 1;
            continue;
        }

        if chars[index] != '$' {
            text.push(chars[index]);
            index += 1;
            continue;
        }

        // `$$` is a literal dollar.
        if chars.get(index + 1) == Some(&'$') {
            text.push('$');
            index += 2;
            continue;
        }

        // `${1:default}`
        if chars.get(index + 1) == Some(&'{') {
            let mut cursor = index + 2;
            let mut number = String::new();
            while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                number.push(chars[cursor]);
                cursor += 1;
            }

            if !number.is_empty() && chars.get(cursor) == Some(&':') {
                cursor += 1;
                let start = text.chars().count();
                let mut depth = 1;
                while cursor < chars.len() {
                    match chars[cursor] {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    text.push(chars[cursor]);
                    cursor += 1;
                }
                let length = text.chars().count() - start;
                found.push((number.parse().unwrap_or(0), start, length));
                index = cursor + 1;
                continue;
            }

            if !number.is_empty() && chars.get(cursor) == Some(&'}') {
                found.push((number.parse().unwrap_or(0), text.chars().count(), 0));
                index = cursor + 1;
                continue;
            }
        }

        // `$1`
        let mut cursor = index + 1;
        let mut number = String::new();
        while cursor < chars.len() && chars[cursor].is_ascii_digit() {
            number.push(chars[cursor]);
            cursor += 1;
        }
        if !number.is_empty() {
            found.push((number.parse().unwrap_or(0), text.chars().count(), 0));
            index = cursor;
            continue;
        }

        // A lone `$` is just a dollar sign.
        text.push('$');
        index += 1;
    }

    // `$0` is the final stop wherever it was written; everything else is in numeric order.
    found.sort_by_key(|(number, _, _)| if *number == 0 { u32::MAX } else { *number });
    if !found.iter().any(|(number, _, _)| *number == 0) {
        found.push((0, text.chars().count(), 0));
    }

    Expansion {
        stops: found.iter().map(|(_, at, _)| *at).collect(),
        lengths: found.iter().map(|(_, _, length)| *length).collect(),
        text,
    }
}

/// An in-progress expansion: where the stops ended up, and which one the cursor is on.
///
/// The stops are [`Anchor`]s rather than offsets, so typing at one moves the others correctly — the
/// same machinery selections and diagnostics use.
#[derive(Debug, Clone)]
pub struct Session {
    stops: Vec<(Anchor, usize)>,
    at: usize,
}

impl Session {
    /// Starts a session for an expansion inserted at `origin`. `None` when there is nothing to walk.
    pub fn new(expansion: &Expansion, origin: CharOffset) -> Option<Self> {
        if expansion.stops.len() <= 1 {
            return None;
        }
        Some(Self {
            stops: expansion
                .stops
                .iter()
                .zip(&expansion.lengths)
                .map(|(at, length)| {
                    (
                        // Right bias: text typed *at* a stop belongs to it and pushes it along,
                        // which is what makes filling one in behave like typing.
                        Anchor::new(CharOffset(origin.get() + at), Bias::Right),
                        *length,
                    )
                })
                .collect(),
            at: 0,
        })
    }

    /// The current stop, as a range to select.
    pub fn current(&self) -> Option<(CharOffset, usize)> {
        self.stops
            .get(self.at)
            .map(|(anchor, length)| (anchor.offset, *length))
    }

    /// Moves to the next stop. Returns false when the last one is passed, which ends the session.
    pub fn advance(&mut self) -> bool {
        if self.at + 1 >= self.stops.len() {
            return false;
        }
        self.at += 1;
        true
    }

    /// Moves the stops through an edit, so they still point at the right text afterwards.
    pub fn transform(&mut self, edits: &[Edit]) {
        for (anchor, _) in &mut self.stops {
            *anchor = anchor.transform(edits);
            // Text typed into a placeholder replaces it, so its default no longer applies.
        }
        for (_, length) in &mut self.stops {
            *length = 0;
        }
    }

    pub fn is_finished(&self) -> bool {
        self.at + 1 >= self.stops.len()
    }
}

/// Finds the snippet whose prefix the text before the cursor ends with.
pub fn trigger<'a>(
    before: &str,
    snippets: &'a [dacode_import::Snippet],
) -> Option<&'a dacode_import::Snippet> {
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    if word.is_empty() {
        return None;
    }
    // Longest prefix wins, so `for` does not shadow `foreach`.
    snippets
        .iter()
        .filter(|snippet| snippet.prefix == word)
        .max_by_key(|snippet| snippet.prefix.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_snippet_expands_to_its_text_with_one_stop_at_the_end() {
        let expansion = expand("println!()", "");
        assert_eq!(expansion.text, "println!()");
        assert_eq!(
            expansion.stops,
            vec![10],
            "a body with no tab stops still ends somewhere"
        );
    }

    #[test]
    fn numbered_stops_come_out_in_order_with_zero_last() {
        let expansion = expand("fn $1($2) -> $3 {\n    $0\n}", "");
        assert_eq!(expansion.text, "fn () ->  {\n    \n}");
        assert_eq!(expansion.stops.len(), 4);

        // Ascending, and the final stop is where `$0` was written rather than at the very end.
        assert!(expansion.stops.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(expansion.stops[3], 16);
    }

    #[test]
    fn a_placeholder_keeps_its_default_text_and_reports_its_length() {
        let expansion = expand("for ${1:item} in ${2:items} {}", "");
        assert_eq!(expansion.text, "for item in items {}");
        assert_eq!(expansion.stops[0], 4);
        assert_eq!(
            expansion.lengths[0], 4,
            "`item` is selected, ready to replace"
        );
        assert_eq!(expansion.stops[1], 12);
        assert_eq!(expansion.lengths[1], 5);
    }

    #[test]
    fn continuation_lines_take_the_triggering_indentation() {
        // Without this a snippet used inside a function lands flush against the left margin.
        let expansion = expand("if $1 {\n    $0\n}", "        ");
        assert_eq!(
            expansion.text, "if  {\n            \n        }",
            "each new line starts at the caller's indent"
        );
    }

    #[test]
    fn dollars_can_be_literal() {
        let expansion = expand("echo $$HOME and $ alone", "");
        assert_eq!(expansion.text, "echo $HOME and $ alone");
    }

    #[test]
    fn a_session_walks_the_stops_and_then_ends() {
        let expansion = expand("fn ${1:name}($2) {\n    $0\n}", "");
        let mut session = Session::new(&expansion, CharOffset(100)).expect("several stops");

        let (at, length) = session.current().unwrap();
        assert_eq!(
            at,
            CharOffset(103),
            "offsets are relative to where it was inserted"
        );
        assert_eq!(length, 4, "`name` is selected");

        assert!(session.advance());
        assert!(session.advance());
        assert!(!session.advance(), "past the last stop the session is over");
    }

    #[test]
    fn a_snippet_with_a_single_stop_needs_no_session() {
        let expansion = expand("println!()", "");
        assert!(
            Session::new(&expansion, CharOffset(0)).is_none(),
            "nothing to walk means nothing to keep alive"
        );
    }

    #[test]
    fn stops_move_with_the_text_typed_into_them() {
        // The reason stops are anchors: filling in the first placeholder must not leave the second
        // pointing at the wrong character.
        let expansion = expand("fn $1($2) {}", "");
        let mut session = Session::new(&expansion, CharOffset(0)).expect("stops");

        let second_before = session.stops[1].0.offset;
        session.transform(&[Edit::insert(CharOffset(3), "compute")]);

        assert_eq!(
            session.stops[1].0.offset.get(),
            second_before.get() + 7,
            "the later stop moved by what was typed"
        );
    }

    #[test]
    fn the_trigger_is_the_word_before_the_cursor() {
        let snippets = vec![
            dacode_import::Snippet {
                name: "for loop".into(),
                prefix: "for".into(),
                body: "for $1 {}".into(),
                description: None,
                tab_stops: vec![1],
            },
            dacode_import::Snippet {
                name: "print".into(),
                prefix: "pr".into(),
                body: "println!()".into(),
                description: None,
                tab_stops: vec![],
            },
        ];

        assert_eq!(
            trigger("    for", &snippets).map(|s| &s.name),
            Some(&"for loop".to_string())
        );
        assert_eq!(
            trigger("x.pr", &snippets).map(|s| &s.name),
            Some(&"print".to_string())
        );
        assert!(trigger("nothing", &snippets).is_none());
        assert!(trigger("", &snippets).is_none(), "no word, no trigger");
        assert!(trigger("    ", &snippets).is_none());
    }
}

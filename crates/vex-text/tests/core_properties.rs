//! Property tests for the text core.
//!
//! These are the highest-value tests in the project: everything above this layer assumes undo
//! is exact, anchors track the right character, and coordinate conversions round-trip. A bug
//! here shows up as data loss much later, in a form that is nearly impossible to diagnose.

use proptest::prelude::*;
use vex_text::{Anchor, Bias, CharOffset, Edit, Point, Selection, SelectionSet, Text, Transaction};

/// Deliberately nasty alphabet: multi-byte, combining marks, ZWJ emoji, CJK, RTL, CRLF.
const ALPHABET: &[&str] = &[
    "a",
    "b",
    " ",
    "\n",
    "\r\n",
    "\t",
    "é",
    "e\u{301}",
    "字",
    "👨‍👩‍👧‍👦",
    "🇸🇦",
    "م",
    "\u{200b}",
];

fn arb_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(0..ALPHABET.len(), 0..40)
        .prop_map(|idx| idx.into_iter().map(|i| ALPHABET[i]).collect())
}

/// A sorted, disjoint, in-bounds edit list for a document of `len` chars — i.e. a valid
/// transaction.
fn arb_edits(len: usize) -> impl Strategy<Value = Vec<Edit>> {
    // (gap before, length to replace, replacement) triples, applied left to right.
    proptest::collection::vec((0..=len.max(1), 0..4usize, arb_text()), 0..5).prop_map(
        move |specs| {
            let mut edits = Vec::new();
            let mut cursor = 0usize;
            for (gap, take, replacement) in specs {
                if cursor > len {
                    break;
                }
                let start = (cursor + gap % (len - cursor + 1)).min(len);
                let end = (start + take).min(len);
                edits.push(Edit::replace(
                    CharOffset(start)..CharOffset(end),
                    replacement,
                ));
                cursor = end.max(start);
                if cursor >= len {
                    break;
                }
            }
            edits
        },
    )
}

/// A document together with a valid transaction over it.
fn arb_text_and_edits() -> impl Strategy<Value = (String, Vec<Edit>)> {
    arb_text().prop_flat_map(|s| {
        let len = Text::from_str(&s).len_chars().get();
        (Just(s), arb_edits(len))
    })
}

proptest! {
    /// Undoing every transaction in reverse must restore the original text exactly, and
    /// redoing must return to the edited text. This is the property that protects against
    /// data loss.
    #[test]
    fn undo_redo_round_trips(initial in arb_text(), rounds in 1..6usize) {
        let original = Text::from_str(&initial);
        let mut text = original.clone();
        let mut undo_stack: Vec<Transaction> = Vec::new();
        let mut redo_stack: Vec<Transaction> = Vec::new();

        for round in 0..rounds {
            // Deterministic pseudo-edits derived from the round, so this stays a pure property
            // test without needing a second nested strategy.
            let len = text.len_chars().get();
            let at = CharOffset((round * 7) % (len + 1));
            let tx = Transaction::new(vec![Edit::insert(at, "é字\n")]);
            let inverse = tx.apply(&mut text).unwrap();
            undo_stack.push(inverse);
        }

        let edited = text.clone();

        while let Some(tx) = undo_stack.pop() {
            redo_stack.push(tx.apply(&mut text).unwrap());
        }
        prop_assert_eq!(text.to_string(), original.to_string(), "undo-all must restore original");

        while let Some(tx) = redo_stack.pop() {
            tx.apply(&mut text).unwrap();
        }
        prop_assert_eq!(text.to_string(), edited.to_string(), "redo-all must restore edited");
    }

    /// A transaction's inverse must undo it in one step, for arbitrary multi-edit transactions.
    #[test]
    fn inverse_is_exact((initial, edits) in arb_text_and_edits()) {
        let text = Text::from_str(&initial);
        let mut edited = text.clone();
        let tx = Transaction::new(edits);
        let inverse = tx.apply(&mut edited).unwrap();
        inverse.apply(&mut edited).unwrap();

        prop_assert_eq!(edited.to_string(), text.to_string());
    }

    /// Every coordinate conversion must round-trip for every position a cursor can actually
    /// occupy — i.e. every grapheme boundary. Offsets *inside* a cluster (notably between the
    /// `\r` and `\n` of a CRLF) are not valid positions and are deliberately clamped instead;
    /// see `crlf_interior_is_not_a_position`.
    #[test]
    fn coordinates_round_trip(s in arb_text()) {
        let text = Text::from_str(&s);
        let mut boundaries = vec![CharOffset(0)];
        let mut at = CharOffset(0);
        while at.get() < text.len_chars().get() {
            at = text.next_grapheme(at);
            boundaries.push(at);
        }

        for offset in boundaries {

            let point = text.char_to_point(offset);
            prop_assert_eq!(text.point_to_char(point), offset, "char -> point -> char");

            let byte = text.char_to_byte(offset);
            prop_assert_eq!(text.byte_to_char(byte), offset, "char -> byte -> char");

            let (line, col16) = text.point_to_lsp(point);
            prop_assert_eq!(text.lsp_to_point(line, col16), point, "point -> lsp -> point");

            prop_assert_eq!(text.char_to_line(offset), point.line, "line agreement");
        }
    }

    /// Positions strictly after an edit shift by exactly the edit's delta; positions strictly
    /// before it do not move at all.
    #[test]
    fn anchors_shift_by_delta(s in arb_text(), inserted in arb_text()) {
        let text = Text::from_str(&s);
        let len = text.len_chars().get();
        prop_assume!(len >= 2);

        let at = CharOffset(len / 2);
        let edits = vec![Edit::insert(at, inserted.clone())];
        let delta = inserted.chars().count();

        let before = Anchor::right(CharOffset(0)).transform(&edits);
        prop_assert_eq!(before.offset, CharOffset(0), "anchor before an edit must not move");

        let after = Anchor::left(CharOffset(len)).transform(&edits);
        prop_assert_eq!(after.offset, CharOffset(len + delta), "anchor after must shift by delta");
    }

    /// The character an anchor points at is preserved across an edit that happens elsewhere.
    /// This is the property that keeps diagnostics and marks attached to the right code.
    #[test]
    fn anchor_keeps_its_character(s in arb_text()) {
        let text = Text::from_str(&s);
        let len = text.len_chars().get();
        prop_assume!(len >= 4);

        let watched = CharOffset(len - 1);
        let expected = text.rope().char(watched.get());

        // Insert well before the watched position.
        let edits = vec![Edit::insert(CharOffset(1), "xyz")];
        let moved = Anchor::right(watched).transform(&edits);

        let mut edited = text.clone();
        Transaction::new(edits).apply(&mut edited).unwrap();

        prop_assert!(moved.offset.get() < edited.len_chars().get());
        prop_assert_eq!(edited.rope().char(moved.offset.get()), expected);
    }

    /// Bias is the whole reason anchors are typed: at an insertion point, Left stays put and
    /// Right slides past the new text.
    #[test]
    fn bias_disambiguates_insertion_point(s in arb_text(), inserted in arb_text()) {
        let text = Text::from_str(&s);
        let len = text.len_chars().get();
        prop_assume!(len >= 2);
        prop_assume!(!inserted.is_empty());

        let at = CharOffset(len / 2);
        let edits = vec![Edit::insert(at, inserted.clone())];
        let delta = inserted.chars().count();

        prop_assert_eq!(Anchor::new(at, Bias::Left).transform(&edits).offset, at);
        prop_assert_eq!(
            Anchor::new(at, Bias::Right).transform(&edits).offset,
            CharOffset(at.get() + delta)
        );
    }

    /// The selection-set invariant holds no matter what the caller does: sorted, disjoint,
    /// non-empty, with a valid primary index.
    #[test]
    fn selection_set_invariants(raw in proptest::collection::vec((0..50usize, 0..50usize), 1..8), primary in 0..8usize) {
        let selections = raw
            .iter()
            .map(|&(a, b)| Selection::new(CharOffset(a), CharOffset(b)));
        let set = SelectionSet::from_iter(selections, primary);

        prop_assert!(!set.is_empty(), "a document always has at least one selection");
        prop_assert!(set.primary_index() < set.len());

        let starts: Vec<_> = set.iter().map(|s| s.start()).collect();
        let mut sorted = starts.clone();
        sorted.sort();
        prop_assert_eq!(starts, sorted, "selections must be sorted by start");

        for pair in set.iter().collect::<Vec<_>>().windows(2) {
            prop_assert!(pair[0].end() <= pair[1].start(), "selections must not overlap");
        }
    }

    /// Multi-cursor insertion: replacing every selection with the same text is one valid
    /// transaction, and each cursor ends up after its own inserted text.
    #[test]
    fn multi_cursor_insert_is_one_transaction(s in arb_text()) {
        let mut text = Text::from_str(&s);
        let len = text.len_chars().get();
        prop_assume!(len >= 6);

        let set = SelectionSet::from_iter(
            [
                Selection::caret(CharOffset(1)),
                Selection::caret(CharOffset(len / 2)),
                Selection::caret(CharOffset(len - 1)),
            ],
            0,
        );

        let edits = set.replace_edits("//");
        let tx = Transaction::new(edits.clone());
        prop_assert!(tx.validate(&text).is_ok(), "multi-cursor edits must be a valid transaction");

        let inverse = tx.apply(&mut text).unwrap();
        prop_assert_eq!(text.len_chars().get(), len + 2 * set.len());

        let moved = set.transform(&edits);
        prop_assert_eq!(moved.len(), set.len(), "cursors must not collapse");

        inverse.apply(&mut text).unwrap();
        prop_assert_eq!(text.to_string(), s);
    }

    /// Grapheme motion never splits a cluster and always makes progress.
    #[test]
    fn grapheme_motion_is_sane(s in arb_text()) {
        let text = Text::from_str(&s);
        let len = text.len_chars().get();

        let mut at = CharOffset(0);
        let mut steps = 0;
        while at.get() < len {
            let next = text.next_grapheme(at);
            prop_assert!(next.get() > at.get(), "forward motion must advance");
            prop_assert_eq!(text.prev_grapheme(next), at, "motion must be reversible");
            at = next;
            steps += 1;
            prop_assert!(steps <= len, "must terminate");
        }
        prop_assert_eq!(at, CharOffset(len));
    }
}

#[test]
fn line_lengths_exclude_line_breaks() {
    let text = Text::from_str("abc\r\ndef\nghi");
    assert_eq!(
        text.line_len(0),
        3,
        "CRLF must not count toward line length"
    );
    assert_eq!(text.line_len(1), 3);
    assert_eq!(text.line_len(2), 3);
    assert_eq!(text.char_to_point(CharOffset(5)), Point::new(1, 0));
}

#[test]
fn crlf_interior_is_not_a_position() {
    // A cursor cannot sit between the \r and the \n: CRLF is one grapheme cluster and one line
    // break. Converting such an offset clamps to the end of the line's visible content rather
    // than inventing a column past it.
    let text = Text::from_str("ab\r\ncd");
    assert_eq!(text.line_len(0), 2);
    assert_eq!(text.point_to_char(Point::new(0, 3)), CharOffset(2));
    assert_eq!(
        text.next_grapheme(CharOffset(2)),
        CharOffset(4),
        "CRLF advances as one unit"
    );
    assert_eq!(text.prev_grapheme(CharOffset(4)), CharOffset(2));
}

#[test]
fn lsp_columns_are_utf16() {
    // An astral-plane character is 1 char but 2 UTF-16 code units: exactly the mismatch that
    // silently corrupts LSP positions when a single integer type is used everywhere.
    let text = Text::from_str("a👨b");
    let point = text.char_to_point(CharOffset(2)); // after the emoji
    let (line, col16) = text.point_to_lsp(point);
    assert_eq!(line, 0);
    assert_eq!(col16.get(), 3, "1 for 'a' + 2 for the emoji");
    assert_eq!(text.lsp_to_point(line, col16), point);
}

#[test]
fn overlapping_edits_are_rejected() {
    let text = Text::from_str("hello world");
    let tx = Transaction::new(vec![
        Edit::delete(CharOffset(0)..CharOffset(5)),
        Edit::delete(CharOffset(3)..CharOffset(8)),
    ]);
    assert!(tx.validate(&text).is_err());
}

// ---- column selection --------------------------------------------------------------------------

/// The text every block test uses: three lines of different lengths, so the ragged case is not a
/// separate fixture but the ordinary one.
fn ragged() -> Text {
    Text::from_str("alpha beta\nxy\ngamma delta\n")
}

#[test]
fn a_block_covers_the_same_columns_on_every_line() {
    let text = ragged();
    let set = SelectionSet::column(&text, Point::new(0, 2), Point::new(2, 5));

    assert_eq!(set.len(), 3, "one selection per line the rectangle spans");
    let covered: Vec<String> = set
        .iter()
        .map(|s| text.slice_to_string(s.start()..s.end()))
        .collect();
    assert_eq!(covered, vec!["pha", "", "mma"]);
}

#[test]
fn lines_too_short_for_the_block_still_take_part() {
    // As a caret clamped to their end. That is what makes typing down the right edge of ragged text
    // work — and skipping them would silently drop lines out of an edit the user believes covers
    // everything they dragged over.
    let text = ragged();
    let set = SelectionSet::column(&text, Point::new(0, 6), Point::new(2, 9));

    assert_eq!(set.len(), 3);
    let short = set.iter().nth(1).expect("the middle line");
    assert!(
        short.is_caret(),
        "`xy` is two characters, so there is nothing to select"
    );
    assert_eq!(
        text.char_to_point(short.head),
        Point::new(1, 2),
        "and its caret sits at the end of the line rather than nowhere"
    );

    // Typing into the block is then one transaction across all three lines, which is the whole
    // point of drawing one.
    let edits = set.replace_edits("#");
    assert!(Transaction::new(edits).validate(&text).is_ok());
}

#[test]
fn a_block_drawn_backwards_keeps_its_direction() {
    // So that extending it afterwards grows the side the pointer was on, rather than flipping.
    let text = ragged();
    let set = SelectionSet::column(&text, Point::new(0, 5), Point::new(2, 1));

    assert!(
        set.iter().all(|s| s.is_reversed()),
        "heads on the left, where the pointer finished"
    );
    assert_eq!(
        text.slice_to_string(set.iter().next().unwrap().range()),
        "lpha"
    );
}

#[test]
fn a_block_dragged_upwards_spans_the_same_lines() {
    let text = ragged();
    let down = SelectionSet::column(&text, Point::new(0, 1), Point::new(2, 4));
    let up = SelectionSet::column(&text, Point::new(2, 1), Point::new(0, 4));

    let ranges = |set: &SelectionSet| -> Vec<_> { set.iter().map(|s| s.range()).collect() };
    assert_eq!(
        ranges(&down),
        ranges(&up),
        "a rectangle has no preferred corner"
    );
}

#[test]
fn the_primary_is_the_line_the_pointer_is_on() {
    // The statusline reports the primary, and a block whose primary was always its first line would
    // say you were somewhere you are not.
    let text = ragged();
    let set = SelectionSet::column(&text, Point::new(0, 0), Point::new(2, 3));
    assert_eq!(text.char_to_point(set.primary().head).line, 2);

    let upwards = SelectionSet::column(&text, Point::new(2, 0), Point::new(0, 3));
    assert_eq!(text.char_to_point(upwards.primary().head).line, 0);
}

#[test]
fn a_block_past_the_end_of_the_document_stops_at_the_last_line() {
    // A drag can leave the window, and a rectangle that named lines the file does not have would
    // index past the rope.
    let text = ragged();
    let set = SelectionSet::column(&text, Point::new(0, 0), Point::new(99, 3));
    assert!(set.len() <= text.last_line() + 1);
    for selection in set.iter() {
        assert!(selection.end() <= text.len_chars());
    }
}

#[test]
fn a_block_with_no_width_is_a_column_of_carets() {
    // Which is what alt-dragging straight down means, and is the same thing ctrl-alt-↓ builds.
    let text = ragged();
    let set = SelectionSet::column(&text, Point::new(0, 1), Point::new(2, 1));
    assert_eq!(set.len(), 3);
    assert!(set.iter().all(|s| s.is_caret()));
}

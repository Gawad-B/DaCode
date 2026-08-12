//! Editing and undo semantics.

use proptest::prelude::*;
use vex_buffer::{Document, EditKind};
use vex_text::{CharOffset, Point, Selection, SelectionSet, Transaction};

fn doc(contents: &str) -> Document {
    Document::from_str(contents)
}

#[test]
fn insert_moves_the_caret_after_the_text() {
    let mut document = doc("hello");
    document.set_primary_caret(CharOffset(5));
    document.insert(" world");

    assert_eq!(document.text().to_string(), "hello world");
    assert_eq!(document.selections().primary().head, CharOffset(11));
}

#[test]
fn insert_replaces_a_selection() {
    let mut document = doc("hello world");
    document.set_selections(SelectionSet::single(Selection::new(
        CharOffset(6),
        CharOffset(11),
    )));
    document.insert("there");

    assert_eq!(document.text().to_string(), "hello there");
}

#[test]
fn multi_cursor_insert_is_one_edit_at_every_caret() {
    let mut document = doc("a\nb\nc");
    document.set_selections(SelectionSet::from_iter(
        [
            Selection::caret(CharOffset(0)),
            Selection::caret(CharOffset(2)),
            Selection::caret(CharOffset(4)),
        ],
        0,
    ));
    document.insert("// ");

    assert_eq!(document.text().to_string(), "// a\n// b\n// c");
    assert_eq!(document.selections().len(), 3, "cursors must survive");

    // And it is a single undo, because it was a single transaction.
    assert!(document.undo());
    assert_eq!(document.text().to_string(), "a\nb\nc");
}

#[test]
fn backspace_deletes_whole_grapheme_clusters() {
    // Deleting one `char` of a family emoji would leave broken text on screen. The cursor moves
    // by grapheme, so deletion must too.
    let mut document = doc("ab👨‍👩‍👧‍👦");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.delete_backward();

    assert_eq!(document.text().to_string(), "ab");
}

#[test]
fn backspace_with_adjacent_carets_does_not_produce_overlapping_edits() {
    // Two carets one character apart both want the same character. If the deletion edits are
    // allowed to overlap, the transaction is rejected and the keystroke silently does nothing.
    let mut document = doc("abcdef");
    document.set_selections(SelectionSet::from_iter(
        [
            Selection::caret(CharOffset(2)),
            Selection::caret(CharOffset(3)),
        ],
        0,
    ));
    document.delete_backward();

    assert_eq!(document.text().to_string(), "adef");
}

#[test]
fn delete_forward_at_end_of_document_is_a_no_op() {
    let mut document = doc("abc");
    document.set_primary_caret(CharOffset(3));
    let version = document.version();
    document.delete_forward();

    assert_eq!(document.text().to_string(), "abc");
    assert_eq!(
        document.version(),
        version,
        "a no-op must not bump the version"
    );
}

#[test]
fn newline_carries_indentation() {
    let mut document = doc("    let x = 1;");
    document.set_primary_caret(CharOffset(14));
    document.insert_newline();

    assert_eq!(document.text().to_string(), "    let x = 1;\n    ");
}

#[test]
fn typing_coalesces_into_one_undo_but_movement_breaks_it() {
    let mut document = doc("");
    document.insert("h");
    document.insert("i");
    assert_eq!(
        document.history().depth(),
        1,
        "consecutive typing is one group"
    );

    // Moving the cursor ends the run.
    document.move_grapheme(false, false);
    document.insert("!");
    assert_eq!(document.history().depth(), 2);

    assert!(document.undo());
    assert_eq!(document.text().to_string(), "hi");
    assert!(document.undo());
    assert_eq!(document.text().to_string(), "");
}

#[test]
fn deletions_do_not_coalesce_with_insertions() {
    let mut document = doc("abc");
    document.set_primary_caret(CharOffset(3));
    document.insert("d");
    document.delete_backward();

    assert_eq!(
        document.history().depth(),
        2,
        "insert and delete are separate undos"
    );
    assert!(document.undo());
    assert_eq!(document.text().to_string(), "abcd");
}

#[test]
fn undo_restores_the_selections_that_produced_the_edit() {
    // Undo that leaves the cursor somewhere unrelated feels like a new edit rather than a
    // reversal, and loses your place.
    let mut document = doc("hello world");
    document.set_primary_caret(CharOffset(5));
    document.insert("!!!");
    assert_eq!(document.selections().primary().head, CharOffset(8));

    assert!(document.undo());
    assert_eq!(document.selections().primary().head, CharOffset(5));
}

#[test]
fn redo_replays_exactly() {
    let mut document = doc("one");
    document.set_primary_caret(CharOffset(3));
    document.insert(" two");
    document.end_undo_group();
    document.insert(" three");

    assert!(document.undo());
    assert!(document.undo());
    assert_eq!(document.text().to_string(), "one");

    assert!(document.redo());
    assert_eq!(document.text().to_string(), "one two");
    assert!(document.redo());
    assert_eq!(document.text().to_string(), "one two three");
    assert!(!document.redo(), "nothing left to redo");
}

#[test]
fn a_new_edit_discards_the_redo_stack() {
    let mut document = doc("a");
    document.set_primary_caret(CharOffset(1));
    document.insert("b");
    assert!(document.undo());
    document.insert("c");

    assert!(!document.history().can_redo());
    assert_eq!(document.text().to_string(), "ac");
}

#[test]
fn version_increments_on_every_edit_and_undo() {
    // Background results are validated against this number, so a missed increment means stale
    // data gets applied as if it were fresh.
    let mut document = doc("x");
    let start = document.version();

    document.set_primary_caret(CharOffset(1));
    document.insert("y");
    let after_edit = document.version();
    assert!(after_edit > start);

    document.undo();
    assert!(document.version() > after_edit, "undo is a new version too");
}

#[test]
fn snapshots_are_independent_of_later_edits() {
    // The whole basis of off-thread work: a snapshot handed to a service must not change under
    // it while the user keeps typing.
    let mut document = doc("fn main() {}");
    let (snapshot, version) = document.snapshot();

    document.set_primary_caret(CharOffset(12));
    document.insert("\n// more");

    assert_eq!(snapshot.to_string(), "fn main() {}");
    assert!(document.version() > version);
    assert!(
        !version.is_fresh_for(document.version()),
        "snapshot is now stale"
    );
}

#[test]
fn add_caret_below_creates_a_column_of_cursors() {
    let mut document = doc("aaa\nbbb\nccc");
    document.set_primary_caret(CharOffset(1));
    document.add_caret_vertically(1);
    document.add_caret_vertically(1);

    assert_eq!(document.selections().len(), 3);
    document.insert("-");
    assert_eq!(document.text().to_string(), "a-aa\nb-bb\nc-cc");
}

#[test]
fn escape_collapses_to_a_single_cursor() {
    let mut document = doc("aaa\nbbb");
    document.set_primary_caret(CharOffset(1));
    document.add_caret_vertically(1);
    assert_eq!(document.selections().len(), 2);

    document.collapse_selections();
    assert_eq!(document.selections().len(), 1);
}

#[test]
fn vertical_motion_keeps_the_column_where_possible() {
    let mut document = doc("long line here\nab\nlong line here");
    document.set_primary_caret(CharOffset(10));

    document.move_line(1, false);
    // Clamped to the short line...
    assert_eq!(
        document
            .text()
            .char_to_point(document.selections().primary().head),
        Point::new(1, 2)
    );
}

proptest! {
    /// However many edits happen, undoing all of them restores the original text — the property
    /// that makes the editor safe to use on real files.
    #[test]
    fn undo_all_restores_the_original(
        seed in proptest::collection::vec(0..4usize, 1..24),
    ) {
        let original = "fn main() {\n    println!(\"hi\");\n}\n";
        let mut document = Document::from_str(original);

        for step in seed {
            let len = document.text().len_chars().get();
            match step {
                0 => {
                    document.set_primary_caret(CharOffset(len / 2));
                    document.insert("é");
                }
                1 => {
                    document.set_primary_caret(CharOffset(len));
                    document.delete_backward();
                }
                2 => {
                    document.set_primary_caret(CharOffset(0));
                    document.delete_forward();
                }
                _ => {
                    document.set_primary_caret(CharOffset(len / 3));
                    document.insert_newline();
                }
            }
        }

        while document.undo() {}
        prop_assert_eq!(document.text().to_string(), original);
    }
}

#[test]
fn transactions_from_outside_still_go_through_history() {
    // Any code path that bypassed `apply` would break undo. This asserts the funnel works for
    // externally-built transactions too — the shape a formatter or LSP edit will arrive in.
    let mut document = doc("a b c");
    let transaction = Transaction::new(vec![
        vex_text::Edit::replace(CharOffset(0)..CharOffset(1), "X"),
        vex_text::Edit::replace(CharOffset(4)..CharOffset(5), "Z"),
    ]);

    document.apply(transaction, EditKind::Discrete).unwrap();
    assert_eq!(document.text().to_string(), "X b Z");

    assert!(document.undo());
    assert_eq!(document.text().to_string(), "a b c");
}

// ---- buffer search ---------------------------------------------------------------------

#[test]
fn search_finds_every_occurrence_in_order() {
    let text = vex_text::Text::from_str("one two one three one");
    let matches = vex_buffer::find_all(&text, "one", &Default::default());

    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].start.get(), 0);
    assert_eq!(matches[1].start.get(), 8);
    assert_eq!(matches[2].start.get(), 18);
    assert!(
        matches.windows(2).all(|w| w[0].start < w[1].start),
        "matches must be in document order"
    );
}

#[test]
fn search_is_smart_case_like_project_search() {
    let text = vex_text::Text::from_str("Total total TOTAL");

    let lower = vex_buffer::find_all(&text, "total", &Default::default());
    assert_eq!(lower.len(), 3, "a lowercase query matches any case");

    let upper = vex_buffer::find_all(&text, "Total", &Default::default());
    assert_eq!(upper.len(), 1, "an uppercase letter means case matters");
    assert_eq!(upper[0].start.get(), 0);
}

#[test]
fn search_offsets_are_char_based_not_byte_based() {
    // The bug this guards: byte offsets from the scan must be converted, or every match after a
    // multi-byte character points at the wrong place.
    let text = vex_text::Text::from_str("héllo 👋 needle here");
    let matches = vex_buffer::find_all(&text, "needle", &Default::default());

    assert_eq!(matches.len(), 1);
    let found = text.slice_to_string(matches[0].clone());
    assert_eq!(found, "needle", "the range must cover exactly the match");
}

#[test]
fn search_finds_multi_byte_queries() {
    let text = vex_text::Text::from_str("a 日本語 b 日本語 c");
    let matches = vex_buffer::find_all(&text, "日本語", &Default::default());
    assert_eq!(matches.len(), 2);
    assert_eq!(text.slice_to_string(matches[1].clone()), "日本語");
}

#[test]
fn an_empty_query_matches_nothing() {
    let text = vex_text::Text::from_str("anything");
    assert!(vex_buffer::find_all(&text, "", &Default::default()).is_empty());
}

#[test]
fn the_match_limit_is_respected() {
    let text = vex_text::Text::from_str(&"x".repeat(5_000));
    let options = vex_buffer::SearchOptions {
        limit: 10,
        ..Default::default()
    };
    assert_eq!(vex_buffer::find_all(&text, "x", &options).len(), 10);
}

#[test]
fn next_and_previous_wrap_around() {
    // A search that stops at the end of the file forces a manual jump back every time.
    let text = vex_text::Text::from_str("one two one three one");
    let matches = vex_buffer::find_all(&text, "one", &Default::default());

    let first = vex_buffer::find_next(&matches, CharOffset(0), true).unwrap();
    assert_eq!(first.start.get(), 8, "forward from 0 skips the match at 0");

    let wrapped = vex_buffer::find_next(&matches, CharOffset(20), true).unwrap();
    assert_eq!(wrapped.start.get(), 0, "forward past the last match wraps");

    let backward = vex_buffer::find_next(&matches, CharOffset(8), false).unwrap();
    assert_eq!(backward.start.get(), 0);

    let wrapped_back = vex_buffer::find_next(&matches, CharOffset(0), false).unwrap();
    assert_eq!(
        wrapped_back.start.get(),
        18,
        "backward from the first wraps"
    );
}

#[test]
fn next_on_no_matches_is_none() {
    assert!(vex_buffer::find_next(&[], CharOffset(0), true).is_none());
}

#[test]
fn the_current_match_index_is_reported() {
    let text = vex_text::Text::from_str("one two one");
    let matches = vex_buffer::find_all(&text, "one", &Default::default());

    assert_eq!(vex_buffer::index_of(&matches, CharOffset(0)), Some(0));
    assert_eq!(vex_buffer::index_of(&matches, CharOffset(9)), Some(1));
    assert_eq!(vex_buffer::index_of(&matches, CharOffset(5)), None);
}

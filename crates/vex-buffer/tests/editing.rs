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
fn search_offsets_survive_multi_byte_lines_above_the_match() {
    // The rewrite that made search scan a line at a time computes offsets relative to the line's
    // own start. Get that base wrong — by a byte count rather than a character count — and every
    // match below the first non-ASCII line silently points somewhere else.
    let text = vex_text::Text::from_str("héllo 👋\n日本語のテキスト\n\nneedle here\nneedle again");
    let matches = vex_buffer::find_all(&text, "needle", &Default::default());

    assert_eq!(matches.len(), 2);
    for found in &matches {
        assert_eq!(
            text.slice_to_string(found.clone()),
            "needle",
            "a range that does not cover its own match means the line base was wrong"
        );
    }
}

#[test]
fn search_finds_matches_at_both_ends_of_a_line() {
    // Line-scoped scanning is where off-by-ones live: the first character of a line and the last
    // one before the newline are the two it would drop.
    let text = vex_text::Text::from_str("xa\nbx\ncxc");
    let matches = vex_buffer::find_all(&text, "x", &Default::default());

    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].start.get(), 0, "first character of the file");
    assert_eq!(matches[1].start.get(), 4, "last character before a newline");
    assert_eq!(matches[2].start.get(), 7, "middle of the last line");
    for found in &matches {
        assert_eq!(text.slice_to_string(found.clone()), "x");
    }
}

#[test]
fn a_query_spanning_a_newline_still_matches() {
    // Line-scoped scanning cannot see across a newline, so a query that contains one takes the
    // whole-document path. It is not reachable from the search prompt, which is a single line —
    // but replace can produce one, and finding nothing would be a silent wrong answer.
    let text = vex_text::Text::from_str("alpha\nbeta\ngamma\nbeta\n");
    let matches = vex_buffer::find_all(&text, "alpha\nbeta", &Default::default());

    assert_eq!(matches.len(), 1);
    assert_eq!(text.slice_to_string(matches[0].clone()), "alpha\nbeta");
}

#[test]
fn searching_a_large_document_does_not_copy_it() {
    // Not an allocation assertion — it is a timing one in disguise. This used to build one String
    // of the whole document, and a second lowercased one, per keystroke; at this size that was
    // enough to be felt. The test's real job is to fail loudly if the whole-document path is ever
    // made the default again, because it takes an order of magnitude longer than the line scan.
    let line = "the quick brown fox jumps over the lazy dog\n";
    let text = vex_text::Text::from_str(&line.repeat(20_000));

    let started = std::time::Instant::now();
    let matches = vex_buffer::find_all(&text, "fox", &Default::default());
    let elapsed = started.elapsed();

    assert_eq!(matches.len(), 10_000, "capped by the default match limit");
    assert_eq!(text.slice_to_string(matches[0].clone()), "fox");
    assert!(
        elapsed < std::time::Duration::from_millis(750),
        "searching 860KB took {elapsed:?}, which is not a keystroke's worth of work"
    );
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

// ---- pairing and indentation -----------------------------------------------------------

use vex_buffer::{PairConfig, TypeAction};

fn config() -> PairConfig {
    PairConfig::default()
}

#[test]
fn typing_an_opener_at_end_of_line_pairs_it() {
    let mut document = doc("call");
    document.set_primary_caret(CharOffset(4));
    document.type_char('(', &config());

    assert_eq!(document.text().to_string(), "call()");
    assert_eq!(
        document.selections().primary().head,
        CharOffset(5),
        "the cursor lands between the halves"
    );
}

#[test]
fn typing_an_opener_before_a_word_does_not_pair() {
    // Typing `(` before an identifier usually means wrapping it, not creating an empty call. A pair
    // that fires when you did not want it costs more keystrokes than it saved.
    let mut document = doc("value");
    document.set_primary_caret(CharOffset(0));
    document.type_char('(', &config());

    assert_eq!(document.text().to_string(), "(value");
}

#[test]
fn typing_a_closer_steps_over_the_one_already_there() {
    let mut document = doc("call()");
    document.set_primary_caret(CharOffset(5));
    let action = document.type_char(')', &config());

    assert_eq!(action, TypeAction::StepOver);
    assert_eq!(document.text().to_string(), "call()", "nothing is inserted");
    assert_eq!(document.selections().primary().head, CharOffset(6));
}

#[test]
fn quotes_do_not_pair_inside_a_word() {
    // `don't` must not become `don''t`, and a Rust lifetime must not become `''a`.
    let mut document = doc("don");
    document.set_primary_caret(CharOffset(3));
    document.type_char('\'', &config());
    assert_eq!(document.text().to_string(), "don'");

    let mut document = doc("let x: &str = a");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.type_char('"', &config());
    assert_eq!(
        document.text().to_string(),
        "let x: &str = a\"",
        "a quote after an identifier is literal"
    );
}

#[test]
fn quotes_pair_in_open_space() {
    let mut document = doc("let s = ");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.type_char('"', &config());

    assert_eq!(document.text().to_string(), "let s = \"\"");
    assert_eq!(document.selections().primary().head, CharOffset(9));
}

#[test]
fn typing_a_pair_around_a_selection_wraps_it() {
    let mut document = doc("wrap me");
    document.set_selections(SelectionSet::single(Selection::new(
        CharOffset(0),
        CharOffset(4),
    )));
    document.type_char('(', &config());

    assert_eq!(document.text().to_string(), "(wrap) me");
}

#[test]
fn backspace_between_an_empty_pair_removes_both() {
    let mut document = doc("call()");
    document.set_primary_caret(CharOffset(5));
    document.delete_backward_pairs(&config());

    assert_eq!(document.text().to_string(), "call");
}

#[test]
fn backspace_next_to_a_non_empty_pair_removes_one_character() {
    let mut document = doc("call(x)");
    document.set_primary_caret(CharOffset(6));
    document.delete_backward_pairs(&config());

    assert_eq!(document.text().to_string(), "call()");
}

#[test]
fn a_newline_carries_the_previous_indentation() {
    let mut document = doc("    let x = 1;");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.insert_newline_smart(&config());

    assert_eq!(document.text().to_string(), "    let x = 1;\n    ");
}

#[test]
fn a_newline_after_an_opener_adds_a_level() {
    let mut document = doc("fn main() {");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.insert_newline_smart(&config());

    assert_eq!(document.text().to_string(), "fn main() {\n    ");
}

#[test]
fn a_newline_between_a_pair_opens_a_block() {
    // The behavior that makes `{` then Enter produce a body with the closer below it.
    let mut document = doc("fn main() {}");
    document.set_primary_caret(CharOffset(11));
    document.insert_newline_smart(&config());

    assert_eq!(document.text().to_string(), "fn main() {\n    \n}");
    let point = document
        .text()
        .char_to_point(document.selections().primary().head);
    assert_eq!(point.line, 1, "the cursor lands on the blank middle line");
    assert_eq!(point.column, 4, "and at the indented column");
}

#[test]
fn a_newline_after_a_colon_indents_for_python() {
    let mut document = doc("def main():");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.insert_newline_smart(&config());

    assert_eq!(document.text().to_string(), "def main():\n    ");
}

#[test]
fn indentation_respects_the_configured_width() {
    let narrow = PairConfig {
        tab_width: 2,
        ..PairConfig::default()
    };
    let mut document = doc("if x {");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.insert_newline_smart(&narrow);

    assert_eq!(document.text().to_string(), "if x {\n  ");
}

#[test]
fn typing_a_closer_alone_on_a_line_dedents_it() {
    // Electric dedent: the one indentation correction people expect an editor to make.
    let mut document = doc("fn main() {\n    let x = 1;\n        ");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.type_char('}', &config());

    assert_eq!(
        document.text().to_string(),
        "fn main() {\n    let x = 1;\n}",
        "the closer aligns with its opener"
    );
}

#[test]
fn a_closer_with_code_before_it_is_left_alone() {
    // Reindenting a line with code on it would move text placed deliberately.
    let mut document = doc("    let v = vec![1, 2");
    let end = document.text().len_chars();
    document.set_primary_caret(end);
    document.type_char(']', &config());

    assert_eq!(document.text().to_string(), "    let v = vec![1, 2]");
}

#[test]
fn pairing_with_several_cursors_is_one_undo() {
    let mut document = doc("a\nb\nc");
    document.set_selections(SelectionSet::from_iter(
        [
            Selection::caret(CharOffset(1)),
            Selection::caret(CharOffset(3)),
            Selection::caret(CharOffset(5)),
        ],
        0,
    ));
    document.type_char('(', &config());

    assert_eq!(document.text().to_string(), "a()\nb()\nc()");
    assert!(document.undo());
    assert_eq!(document.text().to_string(), "a\nb\nc");
}

#[test]
fn a_replaced_document_can_continue_the_old_version_numbering() {
    // Reloading a file changed on disk builds a new Document. Versions must keep increasing: they
    // are how every background consumer tells a fresh result from a stale one, and the LSP protocol
    // requires it outright.
    let mut original = Document::from_str("before\n");
    original.insert("x");
    original.insert("y");
    let last = original.version();

    let mut reloaded = Document::from_str("after\n");
    assert!(
        reloaded.version() < last,
        "a fresh document starts over, which is the problem"
    );

    reloaded.resume_from(last);
    assert!(
        reloaded.version() > last,
        "after resuming, the new document's version is ahead of the old one's"
    );

    // And it keeps increasing from there.
    let resumed = reloaded.version();
    reloaded.insert("z");
    assert!(reloaded.version() > resumed);
}

#[test]
fn a_line_whose_case_folding_moves_its_interior_does_not_panic() {
    // The defect this exists for killed the editor from the search box. The fast path compared the
    // *total* byte length of the folded line against the original, which looks equivalent to
    // "offsets still line up" and is not: `İ` folds from 2 bytes to 3 and `Å` (U+212B) from 3 to 2,
    // so `İxÅ` folds six bytes to six while every offset inside it moves. Searching it for `x` then
    // sliced the original at a byte in the middle of `Å`:
    //
    //     end byte index 4 is not a char boundary; it is inside 'Å' (bytes 3..6 of string)
    //
    // On the UI thread, on a keystroke in the search prompt, taking every unsaved buffer with it.
    let text = vex_text::Text::from_str("\u{130}x\u{212B}\n");
    let matches = vex_buffer::find_all(&text, "x", &Default::default());

    assert_eq!(
        matches,
        vec![vex_text::CharOffset(1)..vex_text::CharOffset(2)],
        "and the answer is right, not merely non-fatal"
    );
}

#[test]
fn case_insensitive_search_still_finds_ordinary_accented_text() {
    // The fix takes the slow path for any character that changes width when folded, so the common
    // case — accented Latin, which folds in place — must not have been pushed onto it wrongly, and
    // the uncommon case must still find its match rather than silently reporting none.
    let text = vex_text::Text::from_str("Caf\u{c9} and CAF\u{c9}\n");
    assert_eq!(
        vex_buffer::find_all(&text, "caf\u{e9}", &Default::default()).len(),
        2,
        "É folds to é in place"
    );

    // `İ` is the width-changing one, and a query that matches it must still be found.
    let text = vex_text::Text::from_str("\u{130}stanbul\n");
    assert_eq!(
        vex_buffer::find_all(&text, "stanbul", &Default::default()),
        vec![vex_text::CharOffset(1)..vex_text::CharOffset(8)],
        "the slow path has to be correct, not just safe"
    );
}

// ---- backspace with more than one caret --------------------------------------------------------

/// A selection set from carets at the given offsets, primary first.
fn carets(offsets: &[usize]) -> SelectionSet {
    SelectionSet::from_iter(
        offsets
            .iter()
            .map(|at| Selection::caret(CharOffset(*at)))
            .collect::<Vec<_>>(),
        0,
    )
}

#[test]
fn only_the_carets_actually_sitting_on_a_pair_delete_both_halves() {
    // The decision used to be taken once, from the *primary* caret, and then applied to every
    // caret. `type_char` does that deliberately — every caret inserts the same text, so following
    // the primary is predictable. It does not carry over to deleting: a caret in the middle of a
    // word ate the character *after* it because a caret somewhere else happened to be inside `()`.
    let mut document = doc("call() and word");
    document.set_selections(carets(&[5, 15]));
    document.delete_backward_pairs(&config());

    assert_eq!(
        document.text().to_string(),
        "call and wor",
        "the pair caret takes both halves, the word caret takes one character"
    );
}

#[test]
fn a_caret_at_the_start_of_the_document_deletes_nothing_rather_than_forwards() {
    // `prev_grapheme(0)` clamps to 0, so the old two-grapheme range became `0..1` and backspace at
    // the very start of the file deleted the character in front of it.
    let mut document = doc("(hello)");
    document.set_selections(carets(&[0, 7]));
    document.delete_backward_pairs(&config());

    assert_eq!(document.text().to_string(), "(hello");
}

#[test]
fn adjacent_carets_delete_rather_than_being_refused_whole() {
    // Two carets one character apart both wanted `n-1..n+1` and `n..n+2` — overlapping edits, which
    // `Transaction::validate` refuses. The result was discarded by a `let _ =`, so backspace did
    // nothing at all for the entire selection set, silently. `edits_for_deletion` clamps the second
    // range against the first, which is why one of them deletes only partially: that is the
    // helper's existing contract for adjacent carets, and it beats doing nothing.
    let mut document = doc("a()b");
    document.set_selections(carets(&[2, 3]));
    document.delete_backward_pairs(&config());

    assert_ne!(
        document.text().to_string(),
        "a()b",
        "the transaction was rejected and backspace silently did nothing"
    );
}

#[test]
fn backspace_deletes_a_selected_range_whoever_else_is_on_a_pair() {
    // The pair branch filtered range selections out entirely and the other branch handled them, so
    // what backspace did to selected text depended on whether an unrelated caret was inside `()`.
    let mut document = doc("call() and word");
    document.set_selections(SelectionSet::from_iter(
        vec![
            Selection::caret(CharOffset(5)),
            Selection::new(CharOffset(11), CharOffset(15)),
        ],
        0,
    ));
    document.delete_backward_pairs(&config());

    assert_eq!(document.text().to_string(), "call and ");
}

#[test]
fn a_query_that_overlaps_itself_reports_each_occurrence_once() {
    // `find_all` advanced one character past the start of each hit, so `aa` in `aaaa` came back as
    // three ranges — 0..2, 1..3, 2..4. They are not only highlighted: the same vector is handed to
    // replace-all, which turns each range into an edit, and overlapping edits are exactly what
    // `Transaction::validate` refuses. Replace-all on such a query failed whole and reported "the
    // server sent edits that cannot apply", from a search prompt, with no server involved.
    let text = vex_text::Text::from_str("aaaa");
    let found = vex_buffer::find_all(&text, "aa", &vex_buffer::SearchOptions::default());

    assert_eq!(
        found,
        vec![CharOffset(0)..CharOffset(2), CharOffset(2)..CharOffset(4)]
    );

    // And the ranges are usable as edits, which is the property that actually mattered.
    let edits: Vec<vex_text::Edit> = found
        .iter()
        .map(|range| vex_text::Edit::replace(range.clone(), "b"))
        .collect();
    assert!(Transaction::new(edits).validate(&text).is_ok());
}

#[test]
fn the_case_folding_fallback_does_not_overlap_either() {
    // The slow path, reached only by a line whose lowercasing changes its byte widths. Left on the
    // old character-at-a-time walk it would keep producing the overlapping ranges that break
    // replace — in exactly the files nobody tests with.
    let text = vex_text::Text::from_str("İxxxx");
    let found = vex_buffer::find_all(&text, "xx", &vex_buffer::SearchOptions::default());

    assert_eq!(
        found,
        vec![CharOffset(1)..CharOffset(3), CharOffset(3)..CharOffset(5)]
    );
}

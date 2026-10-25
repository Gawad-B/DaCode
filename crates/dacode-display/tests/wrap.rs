//! Soft wrap and viewport arithmetic.

use dacode_display::{DisplayMap, ScrollAnchor, WrapMode};
use dacode_text::{CharOffset, DisplayPoint, Edit, Text, Transaction};
use proptest::prelude::*;

fn map(columns: usize) -> DisplayMap {
    DisplayMap::new(WrapMode::Word, columns)
}

#[test]
fn a_short_line_is_one_row() {
    let text = Text::from_str("short");
    assert_eq!(map(40).rows_in_line(&text, 0), 1);
}

#[test]
fn an_empty_line_still_occupies_a_row() {
    // Otherwise a blank line has nowhere to put a cursor.
    let text = Text::from_str("a\n\nb");
    let rows = map(40).wrap_line(&text, 1);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_empty());
}

#[test]
fn wrapping_breaks_at_word_boundaries() {
    let text = Text::from_str("the quick brown fox jumps over the lazy dog");
    let rows = map(20).wrap_line(&text, 0);

    assert!(rows.len() > 1);
    for row in &rows {
        let content = text.slice_to_string(row.range.clone());
        assert!(
            content.len() <= 20 + 1,
            "row exceeded the column budget: {content:?}"
        );
        // A word-boundary break must not slice through a word.
        if !row.is_first() {
            assert!(
                !content.starts_with(' '),
                "continuation rows should start at a word: {content:?}"
            );
        }
    }

    // No text may be lost or duplicated by wrapping.
    let rejoined: String = rows
        .iter()
        .map(|row| text.slice_to_string(row.range.clone()))
        .collect();
    assert_eq!(rejoined, "the quick brown fox jumps over the lazy dog");
}

#[test]
fn an_unbreakable_run_breaks_mid_word() {
    // A minified JS line or a long URL has no whitespace. Refusing to break would overflow the
    // viewport, so mid-word is correct here.
    let text = Text::from_str(&"x".repeat(100));
    let rows = map(20).wrap_line(&text, 0);

    assert_eq!(rows.len(), 5);
    let rejoined: String = rows
        .iter()
        .map(|row| text.slice_to_string(row.range.clone()))
        .collect();
    assert_eq!(rejoined.len(), 100, "no characters lost");
}

#[test]
fn wrapping_off_means_one_row_per_line() {
    let text = Text::from_str(&"x".repeat(500));
    let no_wrap = DisplayMap::new(WrapMode::None, 20);
    assert_eq!(no_wrap.rows_in_line(&text, 0), 1);
}

#[test]
fn continuation_rows_are_indented_under_their_line() {
    let text = Text::from_str("        some deeply indented prose that needs to wrap somewhere");
    let rows = map(30).wrap_line(&text, 0);

    assert!(rows.len() > 1);
    assert_eq!(rows[0].indent, 0, "the first row is not inset");
    assert_eq!(
        rows[1].indent, 8,
        "continuations align under the indentation"
    );
}

#[test]
fn tabs_advance_to_the_next_tab_stop() {
    // Measuring a tab as one column would misplace the cursor on every indented line.
    let text = Text::from_str("\t\tx");
    let display = map(80);
    let rows = display.wrap_line(&text, 0);
    let end = rows[0].range.end;

    let point = display
        .display_point(&text, ScrollAnchor::at_line(0), 10, end)
        .expect("cursor is visible");
    assert_eq!(point.column, 9, "two 4-wide tabs plus one char");
}

#[test]
fn rows_from_walks_forward_from_the_anchor() {
    let text = Text::from_str("a\nb\nc\nd\ne");
    let rows = map(40).rows_from(&text, ScrollAnchor::at_line(2), 2);

    assert_eq!(rows.len(), 2);
    assert_eq!(text.slice_to_string(rows[0].range.clone()), "c");
    assert_eq!(text.slice_to_string(rows[1].range.clone()), "d");
}

#[test]
fn rows_from_stops_at_the_end_of_the_document() {
    // Asking for more rows than exist must terminate rather than spin.
    let text = Text::from_str("a\nb");
    let rows = map(40).rows_from(&text, ScrollAnchor::at_line(0), 100);
    assert_eq!(rows.len(), 2);
}

#[test]
fn an_anchor_can_sit_mid_line_so_long_lines_scroll_smoothly() {
    let text = Text::from_str(&"word ".repeat(40));
    let display = map(20);
    let total = display.rows_in_line(&text, 0);
    assert!(total > 3);

    let rows = display.rows_from(
        &text,
        ScrollAnchor {
            line: 0,
            sub_row: 2,
            column: 0,
        },
        2,
    );
    assert_eq!(rows[0].sub_row, 2, "scrolling starts partway into the line");
}

#[test]
fn display_point_is_none_when_the_cursor_is_off_screen() {
    // This is exactly the "should we scroll?" test.
    let text = Text::from_str("a\nb\nc\nd\ne\nf\ng\nh");
    let display = map(40);
    let far = text.point_to_char(dacode_text::Point::new(7, 0));

    assert!(
        display
            .display_point(&text, ScrollAnchor::at_line(0), 3, far)
            .is_none()
    );
    assert!(
        display
            .display_point(&text, ScrollAnchor::at_line(6), 3, far)
            .is_some()
    );
}

#[test]
fn scrolling_to_reveal_moves_the_minimum_amount() {
    let text = Text::from_str(&(0..50).map(|i| format!("line {i}\n")).collect::<String>());
    let display = map(40);
    let anchor = ScrollAnchor::at_line(0);

    // A cursor already comfortably visible must not move the viewport at all.
    let near = text.point_to_char(dacode_text::Point::new(2, 0));
    assert_eq!(
        display.scroll_to_reveal(&text, anchor, 20, near, 3),
        anchor,
        "no scrolling when the cursor is already in view"
    );

    // A cursor below the viewport scrolls just enough to show it with margin.
    let below = text.point_to_char(dacode_text::Point::new(30, 0));
    let scrolled = display.scroll_to_reveal(&text, anchor, 20, below, 3);
    assert!(scrolled.line > anchor.line);
    assert!(
        display.display_point(&text, scrolled, 20, below).is_some(),
        "the cursor must be visible after scrolling"
    );

    // And above.
    let above = text.point_to_char(dacode_text::Point::new(1, 0));
    let back = display.scroll_to_reveal(&text, scrolled, 20, above, 3);
    assert!(back.line < scrolled.line);
    assert!(display.display_point(&text, back, 20, above).is_some());
}

#[test]
fn vertical_motion_follows_wrapped_rows_not_buffer_lines() {
    // The point of display-aware motion: pressing down inside a wrapped paragraph moves one
    // visual row, not past the whole paragraph.
    let text = Text::from_str("the quick brown fox jumps over the lazy dog\nnext line");
    let display = map(20);

    let start = CharOffset(0);
    let down = display.move_vertically(&text, start, 1, None);

    assert_eq!(
        text.char_to_line(down),
        0,
        "still inside the first buffer line"
    );
    assert!(down > start);
}

#[test]
fn vertical_motion_crosses_into_the_next_line_at_the_last_row() {
    let text = Text::from_str("short\nnext line");
    let display = map(40);
    let down = display.move_vertically(&text, CharOffset(0), 1, None);
    assert_eq!(text.char_to_line(down), 1);
}

#[test]
fn vertical_motion_clamps_at_the_document_edges() {
    let text = Text::from_str("a\nb");
    let display = map(40);
    assert_eq!(
        display.move_vertically(&text, CharOffset(0), -1, None),
        CharOffset::ZERO
    );

    let last = text.len_chars();
    assert_eq!(display.move_vertically(&text, last, 1, None), last);
}

#[test]
fn a_click_maps_back_to_the_position_under_it() {
    let text = Text::from_str("hello world\nsecond line");
    let display = map(40);
    let offset = display.offset_at_display_point(
        &text,
        ScrollAnchor::at_line(0),
        10,
        DisplayPoint::new(1, 7),
    );

    assert_eq!(text.char_to_point(offset), dacode_text::Point::new(1, 7));
}

#[test]
fn clicking_past_the_end_of_a_row_clamps_to_it() {
    let text = Text::from_str("ab");
    let display = map(40);
    let offset = display.offset_at_display_point(
        &text,
        ScrollAnchor::at_line(0),
        10,
        DisplayPoint::new(0, 999),
    );
    assert_eq!(offset, CharOffset(2));
}

proptest! {
    /// Wrapping is lossless for any content at any width: the rows of a line always rejoin to
    /// exactly that line. Text silently dropped by the display layer would look like data loss.
    #[test]
    fn wrapping_never_loses_text(
        words in proptest::collection::vec("[a-z]{1,12}", 1..30),
        columns in 4usize..40,
    ) {
        let line = words.join(" ");
        let text = Text::from_str(&line);
        let rows = map(columns).wrap_line(&text, 0);

        let rejoined: String = rows
            .iter()
            .map(|row| text.slice_to_string(row.range.clone()))
            .collect();
        prop_assert_eq!(rejoined, line);
    }

    /// Rows are contiguous and strictly advancing — no gaps, no overlaps, and always progress,
    /// so wrapping cannot loop forever on a pathological line.
    #[test]
    fn rows_tile_the_line_in_order(
        line in "[ a-z]{0,120}",
        columns in 4usize..30,
    ) {
        let text = Text::from_str(&line);
        let rows = map(columns).wrap_line(&text, 0);

        prop_assert!(!rows.is_empty());
        for pair in rows.windows(2) {
            prop_assert_eq!(pair[0].range.end, pair[1].range.start, "rows must be contiguous");
            prop_assert!(pair[1].range.end > pair[1].range.start, "rows must advance");
        }
    }

    /// After scrolling to reveal a position, that position is visible. The property the whole
    /// anchored-viewport design exists to satisfy.
    #[test]
    fn scroll_to_reveal_always_reveals(
        target_line in 0usize..40,
        height in 4usize..24,
    ) {
        let text = Text::from_str(&(0..40).map(|i| format!("line number {i}\n")).collect::<String>());
        let display = map(30);
        let target = text.point_to_char(dacode_text::Point::new(target_line, 0));

        let anchor = display.scroll_to_reveal(&text, ScrollAnchor::at_line(0), height, target, 2);
        prop_assert!(
            display.display_point(&text, anchor, height, target).is_some(),
            "target line {} not visible at height {}",
            target_line,
            height
        );
    }
}

// ---- folding ---------------------------------------------------------------------------------

use dacode_display::{Folds, fold_range};

fn source() -> Text {
    Text::from_str(
        "fn main() {\n    let a = 1;\n\n    let b = 2;\n}\n\nfn other() {\n    body();\n}\n",
    )
}

#[test]
fn a_fold_covers_the_indented_block_below_its_head() {
    let text = source();
    assert_eq!(
        fold_range(&text, 0, 4),
        Some(0..4),
        "a function head folds through its closing brace"
    );
}

#[test]
fn a_blank_line_inside_a_block_does_not_end_it() {
    // The blank line between `let a` and `let b` is inside the function, and a fold that stopped
    // there would fold one statement instead of a body.
    let text = source();
    let fold = fold_range(&text, 0, 4).expect("folds");
    assert!(
        fold.contains(&3),
        "the line after the blank is still inside"
    );
}

#[test]
fn a_line_with_nothing_indented_under_it_does_not_fold() {
    let text = source();
    assert_eq!(fold_range(&text, 1, 4), None, "a statement is not a block");
    assert_eq!(fold_range(&text, 4, 4), None, "nor is a closing brace");
}

#[test]
fn a_trailing_closing_brace_is_folded_with_its_block() {
    // Pure indent folding leaves `}` behind on a line of its own, because it sits at the *head's*
    // indentation. Vim does that, and it reads as a bug in every brace language.
    let text = source();
    assert_eq!(
        fold_range(&text, 6, 4),
        Some(6..8),
        "`fn other` through its brace"
    );

    // Python has no such line, and nothing should be swept in that does not belong.
    let python = Text::from_str("def f():\n    body()\n\ndef g():\n    other()\n");
    assert_eq!(fold_range(&python, 0, 4), Some(0..1));
}

#[test]
fn folded_lines_are_skipped_and_the_head_reports_what_it_hides() {
    let text = source();
    let mut map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    map.folds.close(0..4);

    let rows = map.rows_from(&text, ScrollAnchor::default(), 10);
    let lines: Vec<usize> = rows.iter().map(|row| row.line).collect();

    assert_eq!(
        lines,
        vec![0, 5, 6, 7, 8, 9],
        "the body is hidden, its head and everything after it stay"
    );
    assert_eq!(rows[0].folded, 4, "the head says how many lines it hides");
    assert_eq!(rows[1].folded, 0);
}

#[test]
fn an_anchor_inside_a_fold_is_nudged_out_rather_than_drawing_hidden_text() {
    // Reachable by folding while scrolled into the block. Drawing from inside would show text the
    // fold says is not there.
    let text = source();
    let mut map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    map.folds.close(0..4);

    let rows = map.rows_from(
        &text,
        ScrollAnchor {
            line: 2,
            sub_row: 0,
            column: 0,
        },
        3,
    );
    assert_eq!(rows[0].line, 5, "the first visible line after the fold");
}

#[test]
fn closing_a_fold_absorbs_the_ones_nested_inside_it() {
    let mut folds = Folds::default();
    folds.close(2..4);
    folds.close(1..8);

    assert!(folds.closed_at(1).is_some());
    assert!(
        folds.closed_at(2).is_none(),
        "the inner fold is part of the outer one now, not a second entry"
    );
}

#[test]
fn hidden_lines_are_reported_and_the_head_is_not() {
    let mut folds = Folds::default();
    folds.close(3..6);

    assert!(
        !folds.is_hidden(3),
        "a fold you cannot see cannot be opened"
    );
    assert!(folds.is_hidden(4) && folds.is_hidden(6));
    assert!(!folds.is_hidden(7));
    assert_eq!(folds.next_visible(4), 7);
}

#[test]
fn opening_a_fold_restores_the_rows() {
    let text = source();
    let mut map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    map.folds.close(0..4);
    assert_eq!(map.rows_from(&text, ScrollAnchor::default(), 10).len(), 6);

    assert!(map.folds.open(0));
    assert_eq!(map.rows_from(&text, ScrollAnchor::default(), 10).len(), 10);
    assert!(
        !map.folds.open(0),
        "opening what is already open changes nothing"
    );
}

#[test]
fn sticky_scroll_reports_the_enclosing_block_heads() {
    use dacode_display::enclosing_heads;

    let text = Text::from_str(
        "impl Thing {\n    fn method(&self) {\n        if true {\n            work();\n        }\n    }\n}\n",
    );

    // Inside the `if`, three levels deep.
    assert_eq!(
        enclosing_heads(&text, 3, 4, 8),
        vec![0, 1, 2],
        "outermost first, so they read down the screen in nesting order"
    );

    // At the top level there is nothing to pin.
    assert_eq!(enclosing_heads(&text, 0, 4, 8), Vec::<usize>::new());

    // The depth bound keeps the context from pushing the code off screen.
    assert_eq!(enclosing_heads(&text, 3, 4, 2), vec![1, 2]);
}

#[test]
fn a_blank_line_borrows_the_context_of_the_code_above_it() {
    use dacode_display::enclosing_heads;

    let text = Text::from_str("fn main() {\n    let a = 1;\n\n    let b = 2;\n}\n");
    assert_eq!(
        enclosing_heads(&text, 2, 4, 8),
        vec![0],
        "standing on a blank line inside a function still shows the function"
    );
}

// ---- horizontal scrolling ---------------------------------------------------------------

#[test]
fn a_caret_past_the_right_edge_drags_the_view_to_it() {
    // The defect: with soft wrap off there was no horizontal offset at all, so anything past the
    // viewport's right edge was permanently invisible *and* unreachable — the caret could be there
    // and you could not see it.
    let text = Text::from_str(&format!("{}END\n", "x".repeat(300)));
    let map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    let anchor = ScrollAnchor::at_line(0);

    let near_the_end = CharOffset(299);
    let moved = map.scroll_to_reveal_column(&text, anchor, 80, near_the_end, 8);
    assert!(moved.column > 0, "the view must follow the caret sideways");
    assert!(
        moved.column <= 299,
        "and must not scroll past the caret itself"
    );

    // Back at the start, it comes back.
    let home = map.scroll_to_reveal_column(&text, moved, 80, CharOffset(0), 8);
    assert_eq!(home.column, 0);
}

#[test]
fn a_caret_already_comfortably_visible_does_not_move_the_view() {
    // A view that re-centres on every keystroke is unusable, which is why the reveal is "the least
    // amount that brings it into view" rather than "put it in the middle".
    let text = Text::from_str(&format!("{}\n", "y".repeat(300)));
    let map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    let anchor = ScrollAnchor {
        line: 0,
        sub_row: 0,
        column: 100,
    };

    let inside = map.scroll_to_reveal_column(&text, anchor, 80, CharOffset(140), 8);
    assert_eq!(inside.column, 100, "nothing to do");
}

#[test]
fn soft_wrap_leaves_the_horizontal_offset_alone() {
    // With wrap on there is nothing to the right of the viewport, so the remembered column must not
    // drift while it is invisible — otherwise turning wrap off returns you somewhere you never were.
    let text = Text::from_str(&format!("{}\n", "z".repeat(300)));
    let map = DisplayMap {
        mode: WrapMode::Word,
        columns: 40,
        ..Default::default()
    };
    let anchor = ScrollAnchor {
        line: 0,
        sub_row: 0,
        column: 77,
    };

    let after = map.scroll_to_reveal_column(&text, anchor, 40, CharOffset(250), 8);
    assert_eq!(after.column, 77, "remembered, not reset and not moved");
}

#[test]
fn the_horizontal_offset_survives_a_vertical_scroll() {
    // `scroll_to_reveal` builds a fresh anchor, and dropping the column there would snap the view
    // back to the left every time the cursor changed row.
    let doc = Text::from_str(&format!(
        "{}\n{}\n{}\n",
        "a".repeat(200),
        "b".repeat(200),
        "c".repeat(200)
    ));
    let map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    let anchor = ScrollAnchor {
        line: 0,
        sub_row: 0,
        column: 50,
    };

    let after = map.scroll_to_reveal(&doc, anchor, 2, CharOffset(450), 1);
    assert_eq!(after.column, 50, "the vertical reveal must not reset it");
}

// ---- folds against a moving document ------------------------------------------------------

/// Twelve numbered lines, so a fold's range and what it hides are both easy to state.
fn numbered() -> Text {
    Text::from_str(
        &(0..12)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn unwrapped() -> DisplayMap {
    DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    }
}

/// Applies one edit and returns the text on either side of it, the way the editor does.
fn edited(text: &Text, range: std::ops::Range<usize>, replacement: &str) -> (Text, Vec<Edit>) {
    let edits = vec![Edit::replace(
        CharOffset(range.start)..CharOffset(range.end),
        replacement,
    )];
    let mut after = text.clone();
    Transaction::new(edits.clone())
        .apply(&mut after)
        .expect("the edit is in bounds");
    (after, edits)
}

#[test]
fn deleting_lines_above_a_fold_moves_it_down_the_same_number() {
    // The whole point: a fold is a pair of line numbers, and line numbers only mean anything
    // against the text they were taken from.
    let before = numbered();
    let mut map = unwrapped();
    map.folds.close(6..9);

    // Remove lines 0 and 1 entirely.
    let cut = before.line_to_char(2).get();
    let (after, edits) = edited(&before, 0..cut, "");
    map.folds.transform(&before, &after, &edits);

    assert_eq!(
        map.folds.closed_at(4),
        Some(4..7),
        "the same block, two lines higher"
    );
}

#[test]
fn a_shrinking_edit_cannot_leave_a_fold_hiding_the_rest_of_the_file() {
    // The defect this exists for. `is_hidden` asks only whether a line falls inside the range, so
    // a fold whose end had run off the end of the document hid *everything below its head*: delete
    // a function above a closed fold and the rest of the file silently disappeared, with a fold
    // marker on one line as the only clue it was still there.
    let before = numbered();
    let mut map = unwrapped();
    map.folds.close(8..11);

    // Cut lines 0..8 — more than enough to run the old range past the new last line.
    let cut = before.line_to_char(8).get();
    let (after, edits) = edited(&before, 0..cut, "");
    map.folds.transform(&before, &after, &edits);

    let last = after.last_line();
    for line in 0..=last {
        assert!(
            !map.folds.is_hidden(line) || line <= 3,
            "line {line} of {last} is hidden by a fold that should have moved with it"
        );
    }
    assert_eq!(map.folds.closed_at(0), Some(0..3));
}

#[test]
fn a_fold_whose_block_was_deleted_is_dropped_rather_than_left_pointing_at_nothing() {
    // Both anchors land on the same offset, so there is nothing left to hide — and a fold marker
    // on a line with no block under it is a control that does nothing when clicked.
    let before = numbered();
    let mut map = unwrapped();
    map.folds.close(4..7);

    let (start, end) = (before.line_to_char(4).get(), before.line_to_char(8).get());
    let (after, edits) = edited(&before, start..end, "");
    map.folds.transform(&before, &after, &edits);

    assert!(map.folds.is_empty(), "nothing left to fold");
}

#[test]
fn typing_on_a_line_below_a_fold_leaves_it_where_it_was() {
    // The common case, and the one a coarser fix — clearing every fold on any edit — would have
    // got wrong: folding a block and then typing anywhere else must not unfold it.
    let before = numbered();
    let mut map = unwrapped();
    map.folds.close(2..5);

    let at = before.line_to_char(9).get();
    let (after, edits) = edited(&before, at..at, "x");
    map.folds.transform(&before, &after, &edits);

    assert_eq!(map.folds.closed_at(2), Some(2..5));
}

#[test]
fn moving_down_from_a_fold_head_lands_after_the_block_it_hides() {
    // Arrow-down used to walk the caret into hidden lines, one row at a time. The caret sat on
    // text nobody could see, and typing then edited it.
    let text = numbered();
    let mut map = unwrapped();
    map.folds.close(3..7);

    let head = text.line_to_char(3);
    let moved = map.move_vertically(&text, head, 1, Some(0));
    assert_eq!(
        text.char_to_line(moved),
        8,
        "the first line the fold does not hide"
    );
}

#[test]
fn moving_up_into_a_fold_lands_on_its_head() {
    let text = numbered();
    let mut map = unwrapped();
    map.folds.close(3..7);

    let below = text.line_to_char(8);
    let moved = map.move_vertically(&text, below, -1, Some(0));
    assert_eq!(text.char_to_line(moved), 3, "the head, not the hidden tail");
}

#[test]
fn the_wheel_steps_over_a_fold_and_can_climb_back_up_over_it() {
    // Scrolling stepped by buffer lines with no idea folds existed, so a notch landed *inside* a
    // closed block — where `rows_from` nudges the anchor forward again to avoid drawing hidden
    // text. The view could go down past a fold and never come back up over it.
    let text = numbered();
    let mut map = unwrapped();
    map.folds.close(2..8);

    let top = ScrollAnchor::at_line(0);
    let down = map.step(&text, top, 3);
    assert_eq!(
        down.line, 9,
        "three rows: line 1, the fold's one row, then line 9"
    );

    let back = map.step(&text, down, -3);
    assert_eq!(back.line, 0, "and the same three rows in reverse");
}

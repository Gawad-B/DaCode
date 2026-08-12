//! Soft wrap and viewport arithmetic.

use proptest::prelude::*;
use vex_display::{DisplayMap, ScrollAnchor, WrapMode};
use vex_text::{CharOffset, DisplayPoint, Text};

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
    let far = text.point_to_char(vex_text::Point::new(7, 0));

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
    let near = text.point_to_char(vex_text::Point::new(2, 0));
    assert_eq!(
        display.scroll_to_reveal(&text, anchor, 20, near, 3),
        anchor,
        "no scrolling when the cursor is already in view"
    );

    // A cursor below the viewport scrolls just enough to show it with margin.
    let below = text.point_to_char(vex_text::Point::new(30, 0));
    let scrolled = display.scroll_to_reveal(&text, anchor, 20, below, 3);
    assert!(scrolled.line > anchor.line);
    assert!(
        display.display_point(&text, scrolled, 20, below).is_some(),
        "the cursor must be visible after scrolling"
    );

    // And above.
    let above = text.point_to_char(vex_text::Point::new(1, 0));
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

    assert_eq!(text.char_to_point(offset), vex_text::Point::new(1, 7));
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
        let target = text.point_to_char(vex_text::Point::new(target_line, 0));

        let anchor = display.scroll_to_reveal(&text, ScrollAnchor::at_line(0), height, target, 2);
        prop_assert!(
            display.display_point(&text, anchor, height, target).is_some(),
            "target line {} not visible at height {}",
            target_line,
            height
        );
    }
}

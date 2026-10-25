//! Motions and text objects resolved against real text.

use dacode_input::{
    Motion, TextObject, line_range, operator_range, resolve_motion, resolve_object,
};
use dacode_text::{CharOffset, Text};
use proptest::prelude::*;

fn at(text: &Text, from: usize, motion: Motion, count: usize) -> usize {
    resolve_motion(text, CharOffset(from), motion, count).get()
}

/// The text a motion or object range covers, for readable assertions.
fn covered(source: &str, range: std::ops::Range<CharOffset>) -> String {
    source
        .chars()
        .skip(range.start.get())
        .take(range.end.get() - range.start.get())
        .collect()
}

#[test]
fn word_forward_stops_at_class_boundaries() {
    // Vim's rule: switching character class starts a new word, so `foo.bar` is three words.
    let source = "foo.bar baz";
    let text = Text::from_str(source);

    assert_eq!(at(&text, 0, Motion::WordForward, 1), 3, "foo -> .");
    assert_eq!(at(&text, 3, Motion::WordForward, 1), 4, ". -> bar");
    assert_eq!(at(&text, 4, Motion::WordForward, 1), 8, "bar -> baz");
}

#[test]
fn big_word_forward_ignores_punctuation() {
    let text = Text::from_str("foo.bar baz");
    assert_eq!(
        at(&text, 0, Motion::WordForwardBig, 1),
        8,
        "foo.bar is one WORD"
    );
}

#[test]
fn word_backward_lands_on_word_starts() {
    let text = Text::from_str("alpha beta gamma");
    assert_eq!(at(&text, 16, Motion::WordBackward, 1), 11);
    assert_eq!(at(&text, 11, Motion::WordBackward, 1), 6);
    assert_eq!(at(&text, 6, Motion::WordBackward, 1), 0);
    assert_eq!(
        at(&text, 0, Motion::WordBackward, 1),
        0,
        "clamps at the start"
    );
}

#[test]
fn word_end_lands_on_the_last_character() {
    let text = Text::from_str("alpha beta");
    assert_eq!(at(&text, 0, Motion::WordEnd, 1), 4, "the 'a' of alpha");
    assert_eq!(at(&text, 4, Motion::WordEnd, 1), 9, "the 'a' of beta");
}

#[test]
fn a_count_repeats_a_motion() {
    let text = Text::from_str("one two three four");
    assert_eq!(at(&text, 0, Motion::WordForward, 3), 14);
}

#[test]
fn line_motions_stay_on_their_line() {
    let text = Text::from_str("    indented line\nsecond");
    assert_eq!(at(&text, 10, Motion::LineStart, 1), 0);
    assert_eq!(at(&text, 10, Motion::FirstNonBlank, 1), 4);
    assert_eq!(at(&text, 4, Motion::LineEnd, 1), 17);
}

#[test]
fn find_char_searches_within_the_line_only() {
    // A find that jumps lines makes `dt)` unpredictable.
    let source = "call(a, b)\nnext(c)";
    let text = Text::from_str(source);

    assert_eq!(
        at(
            &text,
            0,
            Motion::FindChar {
                target: ')',
                forward: true,
                till: false
            },
            1
        ),
        9
    );
    // No `;` on this line, so the cursor must not move.
    assert_eq!(
        at(
            &text,
            0,
            Motion::FindChar {
                target: ';',
                forward: true,
                till: false
            },
            1
        ),
        0
    );
}

#[test]
fn till_stops_one_short_of_the_target() {
    let text = Text::from_str("abc,def");
    assert_eq!(
        at(
            &text,
            0,
            Motion::FindChar {
                target: ',',
                forward: true,
                till: true
            },
            1
        ),
        2
    );
    assert_eq!(
        at(
            &text,
            6,
            Motion::FindChar {
                target: ',',
                forward: false,
                till: true
            },
            1
        ),
        4
    );
}

#[test]
fn matching_bracket_respects_nesting() {
    let source = "foo(bar(baz), qux)";
    let text = Text::from_str(source);

    // From the outer '(' to the outer ')', not the inner one.
    assert_eq!(at(&text, 3, Motion::MatchingBracket, 1), 17);
    assert_eq!(at(&text, 7, Motion::MatchingBracket, 1), 11);
    // And backward from a closing bracket.
    assert_eq!(at(&text, 17, Motion::MatchingBracket, 1), 3);
}

#[test]
fn paragraph_motion_moves_between_blank_lines() {
    let source = "one\ntwo\n\nthree\nfour\n\nfive";
    let text = Text::from_str(source);
    let forward = at(&text, 0, Motion::ParagraphForward, 1);
    assert_eq!(text.char_to_line(CharOffset(forward)), 2, "the blank line");

    let back = at(&text, forward, Motion::ParagraphBackward, 1);
    assert!(back < forward);
}

#[test]
fn exclusive_and_inclusive_operator_ranges_differ() {
    // `dw` leaves the landing character; `de` takes it. This one distinction is most of what
    // makes operators feel right.
    let source = "alpha beta";
    let text = Text::from_str(source);
    let from = CharOffset(0);

    let word = resolve_motion(&text, from, Motion::WordForward, 1);
    assert_eq!(
        covered(
            source,
            operator_range(&text, from, word, Motion::WordForward)
        ),
        "alpha "
    );

    let end = resolve_motion(&text, from, Motion::WordEnd, 1);
    assert_eq!(
        covered(source, operator_range(&text, from, end, Motion::WordEnd)),
        "alpha"
    );
}

#[test]
fn a_linewise_motion_takes_whole_lines() {
    let source = "one\ntwo\nthree\n";
    let text = Text::from_str(source);
    let from = CharOffset(1); // inside "one"
    let to = resolve_motion(&text, from, Motion::ToLine(1), 1);

    assert_eq!(
        covered(source, operator_range(&text, from, to, Motion::ToLine(1))),
        "one\ntwo\n",
        "dj must take both lines entirely, not a character range"
    );
}

#[test]
fn a_backward_motion_still_yields_an_ordered_range() {
    let source = "alpha beta";
    let text = Text::from_str(source);
    let from = CharOffset(6);
    let to = resolve_motion(&text, from, Motion::WordBackward, 1);

    let range = operator_range(&text, from, to, Motion::WordBackward);
    assert!(range.start <= range.end);
    assert_eq!(covered(source, range), "alpha ");
}

#[test]
fn dd_removes_the_line_including_its_newline() {
    // Otherwise `dd` leaves a blank line behind.
    let source = "one\ntwo\nthree\n";
    let text = Text::from_str(source);
    assert_eq!(covered(source, line_range(&text, 1, 1)), "two\n");
}

#[test]
fn dd_on_the_last_line_does_not_run_past_the_end() {
    let source = "one\ntwo";
    let text = Text::from_str(source);
    let range = line_range(&text, 1, 1);
    assert_eq!(covered(source, range.clone()), "two");
    assert!(range.end <= text.len_chars());
}

#[test]
fn inner_and_around_word_objects() {
    let source = "alpha beta gamma";
    let text = Text::from_str(source);

    let inner = resolve_object(
        &text,
        CharOffset(7),
        TextObject::Word {
            around: false,
            big: false,
        },
    )
    .expect("iw resolves");
    assert_eq!(covered(source, inner), "beta");

    let around = resolve_object(
        &text,
        CharOffset(7),
        TextObject::Word {
            around: true,
            big: false,
        },
    )
    .expect("aw resolves");
    assert_eq!(
        covered(source, around),
        "beta ",
        "aw takes trailing whitespace"
    );
}

#[test]
fn word_objects_work_from_anywhere_inside_the_word() {
    let source = "alpha beta gamma";
    let text = Text::from_str(source);
    for cursor in 6..10 {
        let range = resolve_object(
            &text,
            CharOffset(cursor),
            TextObject::Word {
                around: false,
                big: false,
            },
        )
        .unwrap();
        assert_eq!(covered(source, range), "beta", "from offset {cursor}");
    }
}

#[test]
fn pair_objects_find_the_enclosing_delimiters() {
    let source = "foo(bar(baz), qux)";
    let text = Text::from_str(source);

    // From inside the inner call, `i(` takes the inner contents.
    let inner = resolve_object(
        &text,
        CharOffset(9),
        TextObject::Pair {
            open: '(',
            close: ')',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(covered(source, inner), "baz");

    // From the outer level, it takes everything between the outer pair.
    let outer = resolve_object(
        &text,
        CharOffset(13),
        TextObject::Pair {
            open: '(',
            close: ')',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(covered(source, outer), "bar(baz), qux");
}

#[test]
fn around_pair_includes_the_delimiters() {
    let source = "call(arg)";
    let text = Text::from_str(source);
    let range = resolve_object(
        &text,
        CharOffset(6),
        TextObject::Pair {
            open: '(',
            close: ')',
            around: true,
        },
    )
    .unwrap();
    assert_eq!(covered(source, range), "(arg)");
}

#[test]
fn an_unmatched_pair_resolves_to_nothing() {
    // Better to do nothing than to delete to the end of the file.
    let text = Text::from_str("foo(bar");
    assert!(
        resolve_object(
            &text,
            CharOffset(5),
            TextObject::Pair {
                open: '(',
                close: ')',
                around: false
            }
        )
        .is_none()
    );
}

#[test]
fn quote_objects_pick_the_pair_containing_the_cursor() {
    let source = "let s = \"hello\", t = \"world\";";
    let text = Text::from_str(source);

    let first = resolve_object(
        &text,
        CharOffset(10),
        TextObject::Quote {
            delimiter: '"',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(covered(source, first), "hello");

    let second = resolve_object(
        &text,
        CharOffset(23),
        TextObject::Quote {
            delimiter: '"',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(covered(source, second), "world");
}

#[test]
fn paragraph_objects_span_to_the_blank_lines() {
    let source = "one\ntwo\n\nthree\n";
    let text = Text::from_str(source);
    let range = resolve_object(
        &text,
        CharOffset(0),
        TextObject::Paragraph { around: false },
    )
    .unwrap();
    assert_eq!(covered(source, range), "one\ntwo\n");
}

proptest! {
    /// Motions never leave the document, whatever the count. An out-of-bounds position would
    /// panic on the next slice.
    #[test]
    fn motions_stay_in_bounds(
        source in "[a-z (){}\"\\n]{0,200}",
        from in 0usize..200,
        count in 1usize..20,
        motion_index in 0..12usize,
    ) {
        let text = Text::from_str(&source);
        let len = text.len_chars().get();
        let from = CharOffset(from.min(len));

        let motion = [
            Motion::Left, Motion::Right, Motion::WordForward, Motion::WordBackward,
            Motion::WordEnd, Motion::LineStart, Motion::LineEnd, Motion::FirstNonBlank,
            Motion::FileStart, Motion::FileEnd, Motion::MatchingBracket,
            Motion::ParagraphForward,
        ][motion_index];

        let to = resolve_motion(&text, from, motion, count);
        prop_assert!(to.get() <= len, "{motion:?} left the document: {to:?} > {len}");
    }

    /// Operator ranges are always ordered and in bounds, for any motion from any position.
    #[test]
    fn operator_ranges_are_well_formed(
        source in "[a-z ,.\\n]{1,120}",
        from in 0usize..120,
        motion_index in 0..5usize,
    ) {
        let text = Text::from_str(&source);
        let len = text.len_chars().get();
        let from = CharOffset(from.min(len));

        let motion = [
            Motion::WordForward, Motion::WordBackward, Motion::WordEnd,
            Motion::LineEnd, Motion::Down,
        ][motion_index];

        let to = resolve_motion(&text, from, motion, 1);
        let range = operator_range(&text, from, to, motion);

        prop_assert!(range.start <= range.end, "inverted range for {motion:?}");
        prop_assert!(range.end.get() <= len, "range past the end for {motion:?}");
    }

    /// Text objects, when they resolve at all, resolve to an ordered in-bounds range.
    #[test]
    fn text_objects_are_well_formed(
        source in "[a-z (){}\"'\\n]{1,120}",
        cursor in 0usize..120,
        object_index in 0..5usize,
    ) {
        let text = Text::from_str(&source);
        let len = text.len_chars().get();
        let cursor = CharOffset(cursor.min(len.saturating_sub(1)));

        let object = [
            TextObject::Word { around: false, big: false },
            TextObject::Word { around: true, big: true },
            TextObject::Pair { open: '(', close: ')', around: false },
            TextObject::Quote { delimiter: '"', around: true },
            TextObject::Paragraph { around: false },
        ][object_index];

        if let Some(range) = resolve_object(&text, cursor, object) {
            prop_assert!(range.start <= range.end, "inverted range for {object:?}");
            prop_assert!(range.end.get() <= len, "range past the end for {object:?}");
        }
    }
}

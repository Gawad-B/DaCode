//! The modal grammar: counts, registers, operators, motions, text objects, dot-repeat.

use proptest::prelude::*;
use vex_input::{
    Action, Grammar, InsertPosition, Key, Motion, Operator, Resolution, Target, TextObject,
};
use vex_theme::Mode;

/// Feeds a key sequence and returns every Action it produced.
fn actions(mode: Mode, keys: &str) -> Vec<Action> {
    let mut grammar = Grammar::new();
    Key::parse_sequence(keys)
        .into_iter()
        .filter_map(|key| match grammar.feed(mode, key) {
            Resolution::Action(action) => Some(action),
            _ => None,
        })
        .collect()
}

/// The single Action a sequence resolves to.
fn action(mode: Mode, keys: &str) -> Action {
    let produced = actions(mode, keys);
    assert_eq!(
        produced.len(),
        1,
        "expected {keys:?} to resolve to exactly one action, got {produced:?}"
    );
    produced.into_iter().next().unwrap()
}

fn normal(keys: &str) -> Action {
    action(Mode::Normal, keys)
}

#[test]
fn a_bare_motion_moves() {
    assert_eq!(
        normal("w"),
        Action::Move {
            motion: Motion::WordForward,
            count: 1
        }
    );
}

#[test]
fn a_count_prefixes_a_motion() {
    assert_eq!(
        normal("3w"),
        Action::Move {
            motion: Motion::WordForward,
            count: 3
        }
    );
    assert_eq!(
        normal("12j"),
        Action::Move {
            motion: Motion::Down,
            count: 12
        }
    );
}

#[test]
fn an_operator_waits_for_its_motion() {
    let mut grammar = Grammar::new();
    let pending = grammar.feed(Mode::Normal, Key::char('d'));
    assert!(matches!(pending, Resolution::Pending(_)));
    assert_eq!(
        grammar.pending(),
        "d",
        "the statusline must show the prefix"
    );

    let resolved = grammar.feed(Mode::Normal, Key::char('w'));
    assert_eq!(
        resolved,
        Resolution::Action(Action::Apply {
            operator: Operator::Delete,
            target: Target::Motion {
                motion: Motion::WordForward,
                count: 1
            },
            register: None,
        })
    );
    assert!(grammar.is_idle(), "resolving must clear the prefix");
}

#[test]
fn counts_before_and_after_an_operator_multiply() {
    // `2d3w` deletes six words, as in Vim. This is the case that breaks when a count is bolted
    // on as a flag rather than modelled as a prefix in both positions.
    for keys in ["6dw", "d6w", "2d3w", "3d2w"] {
        assert_eq!(
            normal(keys),
            Action::Apply {
                operator: Operator::Delete,
                target: Target::Motion {
                    motion: Motion::WordForward,
                    count: 6
                },
                register: None,
            },
            "{keys} should delete six words"
        );
    }
}

#[test]
fn a_doubled_operator_is_linewise() {
    assert_eq!(
        normal("dd"),
        Action::Apply {
            operator: Operator::Delete,
            target: Target::WholeLine { count: 1 },
            register: None,
        }
    );
    assert_eq!(
        normal("3yy"),
        Action::Apply {
            operator: Operator::Yank,
            target: Target::WholeLine { count: 3 },
            register: None,
        }
    );
}

#[test]
fn registers_are_a_first_class_prefix() {
    assert_eq!(
        normal("\"ayy"),
        Action::Apply {
            operator: Operator::Yank,
            target: Target::WholeLine { count: 1 },
            register: Some('a'),
        }
    );
    // And they compose with counts and motions in either order.
    assert_eq!(
        normal("\"b2dw"),
        Action::Apply {
            operator: Operator::Delete,
            target: Target::Motion {
                motion: Motion::WordForward,
                count: 2
            },
            register: Some('b'),
        }
    );
}

#[test]
fn text_objects_resolve_after_an_operator() {
    assert_eq!(
        normal("diw"),
        Action::Apply {
            operator: Operator::Delete,
            target: Target::Object(TextObject::Word {
                around: false,
                big: false
            }),
            register: None,
        }
    );
    assert_eq!(
        normal("ca("),
        Action::Apply {
            operator: Operator::Change,
            target: Target::Object(TextObject::Pair {
                open: '(',
                close: ')',
                around: true
            }),
            register: None,
        }
    );
    assert_eq!(
        normal("yi\""),
        Action::Apply {
            operator: Operator::Yank,
            target: Target::Object(TextObject::Quote {
                delimiter: '"',
                around: false
            }),
            register: None,
        }
    );
}

#[test]
fn i_and_a_are_insert_mode_outside_target_position() {
    // The same keys mean "text object" after an operator and "enter insert" without one. If this
    // is wrong, either `diw` or `i` breaks.
    assert_eq!(normal("i"), Action::InsertAt(InsertPosition::BeforeCursor));
    assert_eq!(normal("a"), Action::InsertAt(InsertPosition::AfterCursor));
    assert_eq!(normal("A"), Action::InsertAt(InsertPosition::LineEnd));
    assert_eq!(normal("o"), Action::InsertAt(InsertPosition::LineBelow));
}

#[test]
fn find_char_waits_for_its_argument() {
    assert_eq!(
        normal("f)"),
        Action::Move {
            motion: Motion::FindChar {
                target: ')',
                forward: true,
                till: false
            },
            count: 1
        }
    );
    assert_eq!(
        normal("2dt,"),
        Action::Apply {
            operator: Operator::Delete,
            target: Target::Motion {
                motion: Motion::FindChar {
                    target: ',',
                    forward: true,
                    till: true
                },
                count: 2
            },
            register: None,
        }
    );
}

#[test]
fn a_leading_zero_is_line_start_but_a_trailing_zero_is_a_digit() {
    // The one place digit handling must know what came before it.
    assert_eq!(
        normal("0"),
        Action::Move {
            motion: Motion::LineStart,
            count: 1
        }
    );
    assert_eq!(
        normal("10j"),
        Action::Move {
            motion: Motion::Down,
            count: 10
        }
    );
}

#[test]
fn g_is_a_prefix() {
    assert_eq!(
        normal("gg"),
        Action::Move {
            motion: Motion::FileStart,
            count: 1
        }
    );
    assert_eq!(
        normal("42gg"),
        Action::Move {
            motion: Motion::ToLine(41),
            count: 42
        }
    );
    assert_eq!(
        normal("G"),
        Action::Move {
            motion: Motion::FileEnd,
            count: 1
        }
    );
}

#[test]
fn escape_cancels_a_pending_command_without_acting() {
    let mut grammar = Grammar::new();
    grammar.feed(Mode::Normal, Key::char('2'));
    grammar.feed(Mode::Normal, Key::char('d'));
    assert_eq!(grammar.pending(), "2d");

    let resolution = grammar.feed(Mode::Normal, Key::named(vex_input::KeyCode::Escape));
    assert_eq!(
        resolution,
        Resolution::Pending(String::new()),
        "escape must cancel, not act"
    );
    assert!(grammar.is_idle());
}

#[test]
fn escape_with_nothing_pending_collapses_selections() {
    let mut grammar = Grammar::new();
    assert_eq!(
        grammar.feed(Mode::Normal, Key::named(vex_input::KeyCode::Escape)),
        Resolution::Action(Action::CollapseSelections)
    );
}

#[test]
fn an_unbound_key_clears_the_prefix() {
    // Otherwise a typo silently accumulates and the next real key does something unexpected.
    let mut grammar = Grammar::new();
    grammar.feed(Mode::Normal, Key::char('2'));
    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('§')),
        Resolution::Unhandled
    );
    assert!(
        grammar.is_idle(),
        "a stray key must not leave a live prefix"
    );
}

#[test]
fn visual_mode_motions_extend_instead_of_moving() {
    assert_eq!(
        action(Mode::Visual, "w"),
        Action::Extend {
            motion: Motion::WordForward,
            count: 1
        }
    );
    assert_eq!(
        action(Mode::Visual, "3e"),
        Action::Extend {
            motion: Motion::WordEnd,
            count: 3
        }
    );
}

#[test]
fn visual_mode_operators_apply_to_the_selection() {
    assert_eq!(
        action(Mode::Visual, "d"),
        Action::Apply {
            operator: Operator::Delete,
            target: Target::Selection,
            register: None,
        }
    );
    assert_eq!(
        action(Mode::Visual, "\"ay"),
        Action::Apply {
            operator: Operator::Yank,
            target: Target::Selection,
            register: Some('a'),
        }
    );
}

#[test]
fn insert_mode_produces_text_not_commands() {
    // `dw` in insert mode is two characters, not a delete.
    let produced = actions(Mode::Insert, "dw");
    assert_eq!(
        produced,
        vec![
            Action::InsertText("d".into()),
            Action::InsertText("w".into())
        ]
    );
}

#[test]
fn replace_mode_overwrites() {
    assert_eq!(action(Mode::Replace, "x"), Action::ReplaceChar('x'));
}

#[test]
fn r_replaces_one_character_without_leaving_normal_mode() {
    assert_eq!(normal("rx"), Action::ReplaceChar('x'));
}

#[test]
fn ctrl_bindings_work_in_every_mode() {
    for mode in [Mode::Normal, Mode::Insert, Mode::Visual, Mode::Replace] {
        assert_eq!(action(mode, "<c-r>"), Action::Redo, "in {mode:?}");
        assert_eq!(action(mode, "<c-s>"), Action::Save, "in {mode:?}");
    }
}

#[test]
fn arrow_keys_respect_visual_mode() {
    assert_eq!(
        action(Mode::Normal, "<right>"),
        Action::Move {
            motion: Motion::Right,
            count: 1
        }
    );
    assert_eq!(
        action(Mode::Visual, "<right>"),
        Action::Extend {
            motion: Motion::Right,
            count: 1
        }
    );
}

#[test]
fn only_changes_are_repeatable() {
    // `.` must be predictable: repeating a motion or a mode switch would make it useless.
    assert!(
        Action::Apply {
            operator: Operator::Delete,
            target: Target::WholeLine { count: 1 },
            register: None
        }
        .is_change()
    );
    assert!(Action::InsertText("x".into()).is_change());
    assert!(Action::DeleteBackward.is_change());

    assert!(
        !Action::Apply {
            operator: Operator::Yank,
            target: Target::WholeLine { count: 1 },
            register: None
        }
        .is_change(),
        "yanking changes nothing, so `.` must not replay it"
    );
    assert!(
        !Action::Move {
            motion: Motion::WordForward,
            count: 1
        }
        .is_change()
    );
    assert!(!Action::EnterMode(Mode::Insert).is_change());
}

#[test]
fn linewise_and_inclusive_motions_are_classified() {
    // These two flags decide what an operator's range covers. `dj` taking a character range
    // instead of two lines is the classic way a Vim clone feels subtly wrong.
    assert!(Motion::Down.is_linewise());
    assert!(Motion::ToLine(3).is_linewise());
    assert!(!Motion::WordForward.is_linewise());

    assert!(Motion::WordEnd.is_inclusive());
    assert!(
        Motion::FindChar {
            target: ')',
            forward: true,
            till: false
        }
        .is_inclusive()
    );
    assert!(
        Motion::FindChar {
            target: ')',
            forward: true,
            till: true
        }
        .is_inclusive(),
        "`t` is inclusive too: `dtx` takes through the character before x"
    );
    assert!(
        !Motion::FindChar {
            target: ')',
            forward: false,
            till: false
        }
        .is_inclusive(),
        "backward finds are exclusive"
    );
    assert!(!Motion::WordForward.is_inclusive());
}

#[test]
fn key_sequences_parse_named_keys() {
    let keys = Key::parse_sequence("d<esc><c-r>");
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0], Key::char('d'));
    assert_eq!(keys[1], Key::named(vex_input::KeyCode::Escape));
    assert_eq!(keys[2], Key::ctrl('r'));
}

proptest! {
    /// The state machine must never panic, whatever it is fed. It sits directly under the
    /// keyboard, so any panic here is a crash mid-edit with unsaved work in memory.
    #[test]
    fn arbitrary_key_sequences_never_panic(
        keys in proptest::collection::vec(
            prop_oneof![
                any::<char>().prop_map(Key::char),
                any::<char>().prop_map(Key::ctrl),
                Just(Key::named(vex_input::KeyCode::Escape)),
                Just(Key::named(vex_input::KeyCode::Enter)),
                Just(Key::named(vex_input::KeyCode::Backspace)),
                Just(Key::named(vex_input::KeyCode::Up)),
            ],
            1..60,
        ),
        mode_index in 0..4usize,
    ) {
        let mode = [Mode::Normal, Mode::Insert, Mode::Visual, Mode::Replace][mode_index];
        let mut grammar = Grammar::new();
        for key in keys {
            let _ = grammar.feed(mode, key);
        }
    }

    /// A count of any size resolves with that exact count — no overflow, no silent clamping.
    #[test]
    fn counts_round_trip(count in 1usize..9999) {
        let keys = format!("{count}w");
        prop_assert_eq!(
            normal(&keys),
            Action::Move { motion: Motion::WordForward, count }
        );
    }

    /// After any sequence of keys, the machine is either idle or showing a non-empty prefix —
    /// never in a state the user cannot see.
    #[test]
    fn pending_display_always_reflects_state(
        keys in proptest::collection::vec(
            prop_oneof![
                Just('d'), Just('c'), Just('y'), Just('2'), Just('w'),
                Just('i'), Just('a'), Just('"'), Just('f'), Just('g'),
            ],
            1..12,
        ),
    ) {
        let mut grammar = Grammar::new();
        for c in keys {
            grammar.feed(Mode::Normal, Key::char(c));
        }
        prop_assert!(grammar.is_idle() || !grammar.pending().is_empty());
    }
}

#[test]
fn search_and_command_keys_resolve() {
    assert_eq!(normal("/"), Action::StartSearch { forward: true });
    assert_eq!(normal("?"), Action::StartSearch { forward: false });
    assert_eq!(normal("n"), Action::RepeatSearch { forward: true });
    assert_eq!(normal("N"), Action::RepeatSearch { forward: false });
    assert_eq!(normal(":"), Action::StartCommand);
}

#[test]
fn opening_a_prompt_is_not_a_repeatable_change() {
    // `.` must not reopen a prompt: it repeats edits, not interactions.
    assert!(!Action::StartSearch { forward: true }.is_change());
    assert!(!Action::StartCommand.is_change());
    assert!(!Action::RepeatSearch { forward: true }.is_change());
}

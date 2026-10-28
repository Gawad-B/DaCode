//! The modal grammar: counts, registers, operators, motions, text objects, dot-repeat.

use dacode_input::{
    Action, Grammar, InsertPosition, Key, KeyCode, Keymap, Motion, Operator, Resolution,
    SyntaxObject, Target, TextObject,
};
use dacode_theme::Mode;
use proptest::prelude::*;

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

/// Feeds a sequence into an existing grammar, so a test can check that state carried over.
fn feed(grammar: &mut Grammar, mode: Mode, keys: &str) -> Vec<Action> {
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
fn structural_objects_compose_with_every_operator() {
    // `daf`, `cia`, `yat`: the objects are only worth having because the grammar treats them like
    // `iw` — any operator, any register — rather than as commands of their own.
    let object = |kind, around| Target::Object(TextObject::Syntax { kind, around });
    assert_eq!(
        normal("daf"),
        Action::Apply {
            operator: Operator::Delete,
            target: object(SyntaxObject::Function, true),
            register: None,
        }
    );
    assert_eq!(
        normal("cia"),
        Action::Apply {
            operator: Operator::Change,
            target: object(SyntaxObject::Argument, false),
            register: None,
        }
    );
    assert_eq!(
        normal("\"ayat"),
        Action::Apply {
            operator: Operator::Yank,
            target: object(SyntaxObject::Type, true),
            register: Some('a'),
        }
    );
    assert_eq!(
        normal("dic"),
        Action::Apply {
            operator: Operator::Delete,
            target: object(SyntaxObject::Comment, false),
            register: None,
        }
    );
}

#[test]
fn in_visual_mode_a_text_object_selects_itself() {
    // `vaa` in Vim: with no operator waiting, `a` and `i` still introduce an object, and the object
    // becomes the selection. Before the structural objects nothing reached this, and `a` in visual
    // mode entered insert mode — which is not what it does in any Vim.
    assert_eq!(
        action(Mode::Visual, "aa"),
        Action::SelectObject(TextObject::Syntax {
            kind: SyntaxObject::Argument,
            around: true,
        })
    );
    assert_eq!(
        action(Mode::Visual, "iw"),
        Action::SelectObject(TextObject::Word {
            around: false,
            big: false,
        })
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

    let resolution = grammar.feed(Mode::Normal, Key::named(dacode_input::KeyCode::Escape));
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
        grammar.feed(Mode::Normal, Key::named(dacode_input::KeyCode::Escape)),
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
    assert_eq!(keys[1], Key::named(dacode_input::KeyCode::Escape));
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
                Just(Key::named(dacode_input::KeyCode::Escape)),
                Just(Key::named(dacode_input::KeyCode::Enter)),
                Just(Key::named(dacode_input::KeyCode::Backspace)),
                Just(Key::named(dacode_input::KeyCode::Up)),
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
                // The three prefixes that set a stage: they returned `Pending` with an empty
                // string, which is exactly the state this property says cannot exist, and the
                // alphabet could not reach them to say so.
                Just('q'), Just('@'), Just('z'),
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

#[test]
fn lsp_keys_follow_the_conventions_people_already_know() {
    // `K` for hover and `gd` for definition are what every Neovim LSP configuration binds; matching
    // them is the whole point of "Neovim's keybindings".
    assert_eq!(normal("K"), Action::Hover);
    assert_eq!(normal("gd"), Action::GotoDefinition);
    assert_eq!(normal("]d"), Action::NextDiagnostic { forward: true });
    assert_eq!(normal("[d"), Action::NextDiagnostic { forward: false });
}

#[test]
fn a_bracket_prefix_shows_itself_and_a_stray_key_clears_it() {
    let mut grammar = Grammar::new();
    grammar.feed(Mode::Normal, Key::char(']'));
    assert_eq!(grammar.pending(), "]");

    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('z')),
        Resolution::Unhandled
    );
    assert!(grammar.is_idle());
}

#[test]
fn adding_a_caret_above_moved_to_g_a_so_k_could_be_hover() {
    assert_eq!(normal("gA"), Action::AddCaretAbove);
    assert_eq!(normal("C"), Action::AddCaretBelow);
}

#[test]
fn lsp_requests_are_not_repeatable_changes() {
    // `.` repeats edits. Repeating a hover would be noise.
    assert!(!Action::Hover.is_change());
    assert!(!Action::GotoDefinition.is_change());
    assert!(!Action::NextDiagnostic { forward: true }.is_change());
}

#[test]
fn a_user_binding_takes_precedence_over_the_builtin_key() {
    let mut grammar = Grammar::new();
    let mut keymap = Keymap::new();
    keymap.bind("normal", "<leader>w", "save").unwrap();
    keymap.bind("normal", "U", "redo").unwrap();
    grammar.set_keymap(keymap);

    // A single-key binding fires immediately.
    assert_eq!(feed(&mut grammar, Mode::Normal, "U"), vec![Action::Redo]);

    // A sequence shows its progress in the pending display, then resolves.
    let mut grammar = grammar.clone();
    assert!(matches!(
        grammar.feed(Mode::Normal, Key::char(' ')),
        Resolution::Pending(_)
    ));
    assert_eq!(grammar.pending(), "<space>");
    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('w')),
        Resolution::Action(Action::Save)
    );
    assert!(grammar.is_idle(), "the binding resolved and reset");
}

#[test]
fn a_failed_binding_prefix_replays_through_the_builtins() {
    // `gx` is bound; `gd` is not. Typing `gd` must still reach the built-in goto-definition
    // rather than being swallowed by the half-matched binding.
    let mut grammar = Grammar::new();
    let mut keymap = Keymap::new();
    keymap.bind("normal", "gx", "search_project").unwrap();
    grammar.set_keymap(keymap);

    assert!(matches!(
        grammar.feed(Mode::Normal, Key::char('g')),
        Resolution::Pending(_)
    ));
    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('d')),
        Resolution::Action(Action::GotoDefinition),
        "the buffered `g` was replayed, not lost"
    );
}

#[test]
fn a_binding_cannot_fire_while_an_operator_is_pending() {
    // The invariant that makes user bindings safe: `d` then the leader key must be the grammar's
    // business, not the keymap's, or the operator would be left dangling.
    let mut grammar = Grammar::new();
    let mut keymap = Keymap::new();
    keymap.bind("normal", "<space>w", "save").unwrap();
    grammar.set_keymap(keymap);

    assert!(matches!(
        grammar.feed(Mode::Normal, Key::char('d')),
        Resolution::Pending(_)
    ));
    // Not Save: the keymap is out of the picture until the operator resolves.
    assert_ne!(
        grammar.feed(Mode::Normal, Key::char(' ')),
        Resolution::Action(Action::Save)
    );
}

#[test]
fn insert_mode_bindings_apply_only_to_keys_that_are_not_text() {
    let mut grammar = Grammar::new();
    let mut keymap = Keymap::new();
    keymap.bind("insert", "<c-s>", "save").unwrap();
    // A letter binding in insert mode is accepted by the keymap but must never shadow typing.
    keymap.bind("insert", "x", "quit").unwrap();
    grammar.set_keymap(keymap);

    assert_eq!(
        grammar.feed(Mode::Insert, Key::ctrl('s')),
        Resolution::Action(Action::Save)
    );
    assert_eq!(
        grammar.feed(Mode::Insert, Key::char('x')),
        Resolution::Action(Action::InsertText("x".into())),
        "typing a letter inserts it, whatever the config says"
    );
}

#[test]
fn resolving_a_command_does_not_discard_the_keymap() {
    // `clear` used to reset the whole grammar, which unbound the user's config after one keystroke.
    let mut grammar = Grammar::new();
    let mut keymap = Keymap::new();
    keymap.bind("normal", "U", "redo").unwrap();
    grammar.set_keymap(keymap);

    assert_eq!(feed(&mut grammar, Mode::Normal, "U"), vec![Action::Redo]);
    assert_eq!(
        feed(&mut grammar, Mode::Normal, "U"),
        vec![Action::Redo],
        "still bound after the first command resolved"
    );
}

// ---- with the modal grammar switched off ------------------------------------------------------

fn flat() -> Grammar {
    let mut grammar = Grammar::new();
    grammar.set_modal(false);
    grammar
}

#[test]
fn letters_always_insert_when_the_grammar_is_off() {
    // The whole point of the switch: someone who turns Vim off expects typing to type.
    let mut grammar = flat();
    for (keys, expected) in [("d", "d"), ("w", "w"), ("i", "i"), (":", ":"), ("0", "0")] {
        assert_eq!(
            feed(&mut grammar, Mode::Normal, keys),
            vec![Action::InsertText(expected.into())],
            "{keys} should insert itself"
        );
    }
}

#[test]
fn the_shortcuts_are_the_ones_every_other_editor_has() {
    let mut grammar = flat();

    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('z')),
        Resolution::Action(Action::Undo)
    );
    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('y')),
        Resolution::Action(Action::Redo)
    );
    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('a')),
        Resolution::Action(Action::SelectAll)
    );
    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('v')),
        Resolution::Action(Action::Paste {
            register: None,
            before: false,
            count: 1
        })
    );
    // Copy and cut act on the selection, which exists whether or not there are modes.
    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('c')),
        Resolution::Action(Action::Apply {
            operator: Operator::Yank,
            target: Target::Selection,
            register: None
        })
    );
    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('f')),
        Resolution::Action(Action::StartSearch { forward: true })
    );
}

#[test]
fn shift_and_an_arrow_extends_the_selection() {
    let mut grammar = flat();

    assert_eq!(
        grammar.feed(Mode::Normal, Key::named(KeyCode::Right)),
        Resolution::Action(Action::Move {
            motion: Motion::Right,
            count: 1
        })
    );
    assert_eq!(
        grammar.feed(Mode::Normal, Key::named(KeyCode::Right).with_shift()),
        Resolution::Action(Action::Extend {
            motion: Motion::Right,
            count: 1
        }),
        "shift extends rather than moving"
    );
}

#[test]
fn the_editors_own_chords_survive_the_grammar_being_off() {
    // ctrl-e, ctrl-w and ctrl-k are DaCode's, not Vim's. Turning the modal grammar off should not cost
    // someone the file tree or the ability to split a window.
    let mut grammar = flat();

    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('e')),
        Resolution::Action(Action::ToggleTree)
    );

    assert!(matches!(
        grammar.feed(Mode::Normal, Key::ctrl('w')),
        Resolution::Pending(_)
    ));
    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('v')),
        Resolution::Action(Action::SplitPane { vertical: true })
    );

    assert!(matches!(
        grammar.feed(Mode::Normal, Key::ctrl('k')),
        Resolution::Pending(_)
    ));
    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('o')),
        Resolution::Action(Action::OpenFolder)
    );
}

#[test]
fn switching_the_grammar_off_abandons_any_half_typed_command() {
    // Otherwise a pending `d` would apply itself to the next letter typed as text.
    let mut grammar = Grammar::new();
    assert!(matches!(
        grammar.feed(Mode::Normal, Key::char('d')),
        Resolution::Pending(_)
    ));

    grammar.set_modal(false);
    assert!(grammar.is_idle(), "the pending operator is gone");
    assert_eq!(
        grammar.feed(Mode::Normal, Key::char('w')),
        Resolution::Action(Action::InsertText("w".into()))
    );
}

#[test]
fn the_modal_grammar_is_unchanged_when_it_is_on() {
    // The switch bypasses the grammar rather than altering it, so everything Vim keeps working.
    let mut grammar = Grammar::new();
    grammar.set_modal(false);
    grammar.set_modal(true);

    assert_eq!(
        feed(&mut grammar, Mode::Normal, "dw"),
        vec![Action::Apply {
            operator: Operator::Delete,
            target: Target::Motion {
                motion: Motion::WordForward,
                count: 1
            },
            register: None
        }]
    );
}

#[test]
fn a_count_after_an_operator_still_names_a_line_for_g() {
    // `d5G` deletes to line five. It deleted to the **end of the file** instead: a count typed
    // after an operator lands in `operator_count`, and `G` only read `count`, so it saw none and
    // fell through to "the whole rest of the document". The same for `d5gg` upwards. Both are
    // ordinary Vim, and both silently destroyed far more than was asked for.
    for keys in ["5G", "d5G", "c5G", "y5G"] {
        let action = normal(keys);
        let motion = match &action {
            Action::Move { motion, .. } | Action::Extend { motion, .. } => *motion,
            Action::Apply {
                target: Target::Motion { motion, .. },
                ..
            } => *motion,
            other => panic!("{keys} resolved to {other:?}"),
        };
        assert_eq!(
            motion,
            Motion::ToLine(4),
            "{keys} means line five, not the end of the file"
        );
    }

    for keys in ["5gg", "d5gg"] {
        let action = normal(keys);
        let motion = match &action {
            Action::Move { motion, .. } => *motion,
            Action::Apply {
                target: Target::Motion { motion, .. },
                ..
            } => *motion,
            other => panic!("{keys} resolved to {other:?}"),
        };
        assert_eq!(motion, Motion::ToLine(4), "{keys} means line five");
    }
}

#[test]
fn g_and_gg_with_no_count_at_all_still_reach_the_ends() {
    // The distinction the fix turns on: "no count" is not "a count of one". Losing it would send
    // `G` to line one instead of to the last line, which is a worse defect than the one being
    // fixed.
    assert!(matches!(
        normal("G"),
        Action::Move {
            motion: Motion::FileEnd,
            ..
        }
    ));
    assert!(matches!(
        normal("gg"),
        Action::Move {
            motion: Motion::FileStart,
            ..
        }
    ));
    assert_eq!(
        normal("dG"),
        Action::Apply {
            operator: Operator::Delete,
            target: Target::Motion {
                motion: Motion::FileEnd,
                count: 1
            },
            register: None,
        }
    );
}

// ---- what the machine leaves behind ----------------------------------------------------------

#[test]
fn a_key_the_grammar_does_not_know_clears_the_prefix_whatever_shape_it_is() {
    // `an_unbound_key_clears_the_prefix` only ever fed a `Char`, which hits the one arm that was
    // already clearing. Two others were not: a ctrl or alt key, and any *named* key — PageUp,
    // Delete, F5. Both returned `Unhandled` with the count still live.
    //
    // What makes it worse than it sounds is that the leak is invisible. The application clears its
    // own copy of the pending string on `Unhandled`, and that copy is what the statusline and the
    // which-key popup read — so the display went blank while the grammar was still holding `d3`,
    // and the next `w` deleted three words with nothing on screen to explain why.
    for stray in [
        Key::ctrl('p'),
        Key::named(KeyCode::PageDown),
        Key::named(KeyCode::Function(5)),
    ] {
        let mut grammar = Grammar::new();
        feed(&mut grammar, Mode::Normal, "d3");
        assert!(!grammar.is_idle(), "the operator and count are live");

        assert_eq!(grammar.feed(Mode::Normal, stray), Resolution::Unhandled);
        assert!(
            grammar.is_idle(),
            "{stray:?} left the grammar holding a prefix the user cannot see"
        );
    }
}

#[test]
fn a_chord_typed_with_ctrl_held_completes_instead_of_firing_a_global_binding() {
    // A VSCode-style chord is typed without letting go of ctrl — that is what makes it a chord.
    // The second key reached the global Ctrl bindings first, so `ctrl-k ctrl-s` saved the file and
    // silently dropped the prefix, and `ctrl-k ctrl-o` jumped backwards instead of opening a
    // folder. Both spellings must reach the same place.
    for second in [Key::char('s'), Key::ctrl('s')] {
        let mut grammar = Grammar::new();
        assert!(matches!(
            grammar.feed(Mode::Normal, Key::ctrl('k')),
            Resolution::Pending(_)
        ));
        assert_eq!(
            grammar.feed(Mode::Normal, second),
            Resolution::Action(Action::ToggleSettings),
            "the chord must win over ctrl-s meaning save"
        );
    }

    // And the same for the window prefix, where `ctrl-w ctrl-q` is the one people type.
    let mut grammar = Grammar::new();
    grammar.feed(Mode::Normal, Key::ctrl('w'));
    assert_eq!(
        grammar.feed(Mode::Normal, Key::ctrl('q')),
        Resolution::Action(Action::ClosePane),
        "ctrl-w ctrl-q closes the pane rather than quitting the editor"
    );
}

#[test]
fn every_prefix_says_on_screen_that_it_is_waiting() {
    // `Pending` with an empty string is a machine waiting for a key while telling the user nothing.
    // Five prefixes did it: `q` and `@` and `z` in the modal grammar, and `ctrl-w` and `ctrl-k`
    // with the grammar off — which are *only* reachable that way, so the popup that exists to teach
    // them never appeared for the people who needed it most.
    let mut grammar = Grammar::new();
    for key in ['q', '@', 'z'] {
        assert!(
            matches!(grammar.feed(Mode::Normal, Key::char(key)), Resolution::Pending(shown) if !shown.is_empty()),
            "`{key}` waits for a second key without saying so"
        );
        assert!(!grammar.pending().is_empty());
        grammar.feed(Mode::Normal, Key::named(KeyCode::Escape));
    }

    let mut grammar = flat();
    for key in ['w', 'k'] {
        assert!(
            matches!(grammar.feed(Mode::Normal, Key::ctrl(key)), Resolution::Pending(shown) if !shown.is_empty()),
            "`ctrl-{key}` waits for a second key without saying so"
        );
        // Against the table rather than a literal: what the popup can offer continuations for is
        // the thing that matters, and a hand-written string would drift from it.
        assert!(
            !dacode_input::commands::continuations(grammar.pending(), false).is_empty(),
            "`ctrl-{key}` shows a prefix the which-key popup has nothing to say about"
        );
        grammar.feed(Mode::Normal, Key::named(KeyCode::Escape));
    }
}

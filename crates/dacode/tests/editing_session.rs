//! End-to-end editing: key sequences in, document text out.
//!
//! These are the tests the `Action` design exists to make possible. They drive the same path the
//! keyboard drives — grammar, then executor — with no GPU and no window, so `3dw` and `ci"` are
//! verified as *behavior* rather than as two components that each look correct alone.

use dacode::chrome::EditorState;
use dacode::execute::{Outcome, execute};
use dacode_buffer::Document;
use dacode_display::{DisplayMap, ScrollAnchor};
use dacode_input::{Action, Grammar, Key, Registers, Resolution};
use dacode_text::{CharOffset, Version};
use dacode_theme::{Mode, Theme, Typography};

/// A headless editor: everything the real one has except a renderer.
struct Session {
    state: EditorState,
    grammar: Grammar,
    registers: Registers,
    last_change: Option<Action>,
}

impl Session {
    fn new(contents: &str) -> Self {
        Self {
            state: EditorState {
                mode: Mode::Normal,
                theme: Theme::walnut(),
                theme_toggled_from: None,
                typography: Typography::default(),
                document: Document::from_str(contents),
                pairs: dacode_buffer::PairConfig::default(),
                format_on_save: false,
                display: DisplayMap::default(),
                scroll: ScrollAnchor::default(),
                path: vec!["test".into()],
                file_path: None,
                pending: String::new(),
                syntax_version: Version(1),
                diagnostics_version: Version(1),
                highlights: Vec::new(),
                node_path: Vec::new(),
                syntax_error: false,
                structure: None,
                frame_ms: 0.0,
                worst_ms: 0.0,
                diagnostics: dacode_lsp::Diagnostics::default(),
                server_status: "test".into(),
                palette: None,
                prompt: None,
                results: None,
                search_matches: Vec::new(),
                hover: None,
                completion: None,
                pending_request: None,
                project_root: std::path::PathBuf::from("."),
                buffer_count: 1,
                buffer_tabs: Vec::new(),
                active_buffer: 0,
                modal: true,
                sticky_scroll: true,
                vertical_goal: None,
                expansions: Default::default(),
                search: None,
                recording: None,
                diff: dacode::git::Diff::default(),
                blame: Default::default(),
                show_diagnostics: false,
                focus: Default::default(),
                terminal: Default::default(),
                problems: Default::default(),
                inlay_hints: Vec::new(),
                show_inlay_hints: false,
                welcome: None,
                settings_page: None,
                settings_active: false,
                tree: None,
                autosave: false,
                line_numbers: dacode::config::LineNumbers::Relative,
                pointer: (0.0, 0.0),
                help: None,
                menu: None,
                chrome: Default::default(),
            },
            grammar: Grammar::new(),
            registers: Registers::new(),
            last_change: None,
        }
    }

    fn at(mut self, offset: usize) -> Self {
        self.state.document.set_primary_caret(CharOffset(offset));
        self
    }

    /// Feeds one key, for the cases that need a modifier held.
    fn press(&mut self, key: Key) -> &mut Self {
        if let Resolution::Action(action) = self.grammar.feed(self.state.mode, key) {
            execute(&mut self.state, &mut self.registers, &action, 40);
        }
        self
    }

    /// Feeds a key sequence, exactly as the binary does.
    fn keys(&mut self, sequence: &str) -> &mut Self {
        for key in Key::parse_sequence(sequence) {
            let resolution = self.grammar.feed(self.state.mode, key);
            if let Resolution::Action(action) = resolution {
                let action = if action == Action::RepeatChange {
                    match self.last_change.clone() {
                        Some(previous) => previous,
                        None => continue,
                    }
                } else {
                    action
                };

                if action.is_change() {
                    self.last_change = Some(action.clone());
                }

                if let Outcome::Quit = execute(&mut self.state, &mut self.registers, &action, 40) {
                    break;
                }
            }
        }
        self
    }

    fn text(&self) -> String {
        self.state.document.text().to_string()
    }

    fn cursor(&self) -> usize {
        self.state.document.selections().primary().head.get()
    }

    fn mode(&self) -> Mode {
        self.state.mode
    }

    fn cursor_line(&self) -> usize {
        self.state.cursor_point().line
    }
}

fn session(contents: &str) -> Session {
    Session::new(contents)
}

#[test]
fn dw_deletes_a_word() {
    let mut s = session("alpha beta gamma").at(0);
    s.keys("dw");
    assert_eq!(s.text(), "beta gamma");
}

#[test]
fn a_count_multiplies_a_delete() {
    let mut s = session("one two three four").at(0);
    s.keys("3dw");
    assert_eq!(s.text(), "four");
}

#[test]
fn dd_deletes_a_whole_line() {
    let mut s = session("first\nsecond\nthird\n").at(6);
    s.keys("dd");
    assert_eq!(s.text(), "first\nthird\n");
}

#[test]
fn de_includes_the_last_character_of_the_word() {
    // The inclusive/exclusive distinction, end to end.
    let mut s = session("alpha beta").at(0);
    s.keys("de");
    assert_eq!(s.text(), " beta");
}

#[test]
fn diw_deletes_the_word_under_the_cursor() {
    let mut s = session("alpha beta gamma").at(7);
    s.keys("diw");
    assert_eq!(s.text(), "alpha  gamma");
}

#[test]
fn daw_takes_the_trailing_space_too() {
    let mut s = session("alpha beta gamma").at(7);
    s.keys("daw");
    assert_eq!(s.text(), "alpha gamma");
}

#[test]
fn ci_quote_changes_a_string_and_enters_insert() {
    let mut s = session("let s = \"hello\";").at(10);
    s.keys("ci\"");
    assert_eq!(s.text(), "let s = \"\";");
    assert_eq!(s.mode(), Mode::Insert, "change must leave you typing");

    s.keys("world");
    assert_eq!(s.text(), "let s = \"world\";");
}

#[test]
fn di_paren_works_from_inside_nested_calls() {
    let mut s = session("foo(bar(baz), qux)").at(9);
    s.keys("di(");
    assert_eq!(s.text(), "foo(bar(), qux)");
}

#[test]
fn dt_deletes_up_to_but_not_including_the_target() {
    let mut s = session("call(a, b);").at(4);
    s.keys("dt;");
    assert_eq!(s.text(), "call;", "dt; takes everything up to the ';'");
}

#[test]
fn yank_then_paste_duplicates() {
    let mut s = session("alpha beta").at(0);
    s.keys("yw");
    assert_eq!(s.text(), "alpha beta", "yank must not modify the text");

    s.keys("p");
    assert_eq!(s.text(), "aalpha lpha beta");
}

#[test]
fn a_named_register_is_independent_of_the_unnamed_one() {
    let mut s = session("one two\n").at(0);
    s.keys("\"ayw"); // yank "one " into register a
    s.keys("wdw"); // delete "two" into the unnamed register
    s.keys("\"ap"); // paste register a

    assert!(
        s.text().contains("one one "),
        "register a should still hold 'one ': {:?}",
        s.text()
    );
}

#[test]
fn dd_then_p_moves_a_line() {
    let mut s = session("first\nsecond\nthird\n").at(0);
    s.keys("dd");
    assert_eq!(s.text(), "second\nthird\n");

    // The register is linewise, so the paste lands on a line boundary rather than mid-line.
    s.keys("p");
    assert_eq!(s.text(), "second\nfirst\nthird\n");
}

#[test]
fn dot_repeats_the_last_change() {
    let mut s = session("one two three four five").at(0);
    s.keys("dw");
    assert_eq!(s.text(), "two three four five");

    s.keys(".");
    assert_eq!(s.text(), "three four five");
    s.keys("..");
    assert_eq!(s.text(), "five");
}

#[test]
fn dot_does_not_repeat_a_motion_or_a_yank() {
    // `.` after a motion must do nothing rather than something surprising.
    let mut s = session("alpha beta").at(0);
    s.keys("w");
    let before = s.text();
    s.keys(".");
    assert_eq!(s.text(), before, "a motion is not a change");

    let mut s = session("alpha beta").at(0);
    s.keys("yw.");
    assert_eq!(s.text(), "alpha beta", "a yank is not a change");
}

#[test]
fn visual_mode_selects_then_deletes() {
    let mut s = session("alpha beta gamma").at(0);
    s.keys("vee"); // select two words
    s.keys("d");
    assert_eq!(s.text(), " gamma");
    assert_eq!(s.mode(), Mode::Normal, "operating exits visual mode");
}

#[test]
fn escape_leaves_insert_mode_and_returns_to_normal() {
    let mut s = session("").at(0);
    s.keys("ihello<esc>");
    assert_eq!(s.text(), "hello");
    assert_eq!(s.mode(), Mode::Normal);

    // And normal-mode keys are commands again, not text.
    s.keys("dd");
    assert_eq!(s.text(), "");
}

#[test]
fn insert_mode_typing_and_backspace() {
    // With auto-pairing, `{` supplies its own `}` and Enter between them opens the body — so the
    // closer is never typed. This test was written before pairing existed and typed it manually,
    // which now produces a second brace.
    let mut s = session("").at(0);
    s.keys("ifn main() {<cr>");
    assert_eq!(
        s.text(),
        "fn main() {\n    \n}",
        "the block opens with the closer already placed"
    );

    s.keys("<esc>");
    s.keys("i<bs><esc>");
    assert_eq!(
        s.text(),
        "fn main() {\n   \n}",
        "backspace removes one space of indentation"
    );
}

#[test]
fn typing_a_call_pairs_the_parentheses() {
    let mut s = session("").at(0);
    s.keys("iprint(<esc>");
    assert_eq!(s.text(), "print()");
}

#[test]
fn typing_the_closer_steps_over_it_rather_than_doubling() {
    let mut s = session("").at(0);
    s.keys("iprint(x)<esc>");
    assert_eq!(
        s.text(),
        "print(x)",
        "the typed `)` steps over the auto-inserted one"
    );
}

#[test]
fn o_opens_a_line_below_with_the_same_indentation() {
    let mut s = session("    let x = 1;").at(4);
    s.keys("o");
    assert_eq!(s.mode(), Mode::Insert);
    s.keys("let y = 2;<esc>");
    assert_eq!(s.text(), "    let x = 1;\n    let y = 2;");
}

#[test]
fn a_appends_after_the_cursor() {
    let mut s = session("ab").at(0);
    s.keys("aX<esc>");
    assert_eq!(s.text(), "aXb");
}

#[test]
fn r_replaces_one_character_and_stays_in_normal_mode() {
    let mut s = session("cat").at(0);
    s.keys("rb");
    assert_eq!(s.text(), "bat");
    assert_eq!(s.mode(), Mode::Normal);
}

#[test]
fn indent_and_outdent_operate_on_whole_lines() {
    let mut s = session("one\ntwo\n").at(0);
    s.keys(">>");
    assert_eq!(s.text(), "    one\ntwo\n");
    s.keys("<<");
    assert_eq!(s.text(), "one\ntwo\n");
}

#[test]
fn case_operators_transform_a_motion_range() {
    let mut s = session("hello world").at(0);
    s.keys("Uw");
    assert_eq!(s.text(), "HELLO world");
}

#[test]
fn undo_reverses_a_whole_command_not_a_keystroke() {
    // `3dw` must be one undo. If the operator produced three transactions, this needs three.
    let mut s = session("one two three four").at(0);
    s.keys("3dw");
    assert_eq!(s.text(), "four");

    s.keys("u");
    assert_eq!(s.text(), "one two three four");
}

#[test]
fn undo_after_a_typing_run_removes_the_run() {
    let mut s = session("").at(0);
    s.keys("ihello<esc>");
    s.keys("u");
    assert_eq!(s.text(), "", "a typing run is one undo");
}

#[test]
fn redo_replays_what_undo_reversed() {
    let mut s = session("alpha beta").at(0);
    s.keys("dw");
    s.keys("u");
    assert_eq!(s.text(), "alpha beta");
    s.keys("<c-r>");
    assert_eq!(s.text(), "beta");
}

#[test]
fn multi_cursor_edits_every_caret() {
    let mut s = session("aaa\nbbb\nccc").at(0);
    s.keys("CC"); // add a caret on each of the next two lines
    assert_eq!(s.state.document.selections().len(), 3);

    s.keys("i// <esc>");
    assert_eq!(s.text(), "// aaa\n// bbb\n// ccc");
}

#[test]
fn multi_cursor_delete_does_not_reject_overlapping_ranges() {
    // Two carets inside the same word both asking for `diw` would produce overlapping edits.
    // Clipping means the keystroke does something sensible instead of nothing.
    let mut s = session("alpha\n").at(1);
    s.keys("C"); // second caret on the (empty) next line
    s.keys("diw");
    assert!(
        !s.text().contains("alpha"),
        "the word should be gone: {:?}",
        s.text()
    );
}

#[test]
fn gg_and_shift_g_jump_to_the_ends_of_the_file() {
    let mut s = session("one\ntwo\nthree\n").at(6);
    s.keys("gg");
    assert_eq!(s.cursor(), 0);
    s.keys("G");
    assert_eq!(
        s.state.document.text().char_to_line(CharOffset(s.cursor())),
        3
    );
}

#[test]
fn a_pending_command_is_visible_and_cancellable() {
    let mut s = session("alpha beta").at(0);
    s.keys("2d");
    assert_eq!(s.grammar.pending(), "2d", "the user must see the prefix");

    s.keys("<esc>");
    assert!(s.grammar.is_idle());
    assert_eq!(s.text(), "alpha beta", "cancelling must not edit");
}

#[test]
fn percent_jumps_between_matching_brackets() {
    let mut s = session("foo(bar(baz), qux)").at(3);
    s.keys("%");
    assert_eq!(s.cursor(), 17);
    s.keys("%");
    assert_eq!(s.cursor(), 3);
}

#[test]
fn d_percent_deletes_a_balanced_span() {
    let mut s = session("call(a, b);").at(4);
    s.keys("d%");
    assert_eq!(
        s.text(),
        "call;",
        "the whole balanced span goes, brackets included"
    );
}

#[test]
fn editing_marks_the_document_dirty_until_saved() {
    let mut s = session("x").at(0);
    assert!(!s.state.document.is_dirty());
    s.keys("x");
    assert!(s.state.document.is_dirty());

    s.state.document.mark_saved();
    assert!(!s.state.document.is_dirty());
}

#[test]
fn every_edit_advances_the_version() {
    // Background results are validated against this, so a missed bump means stale data applied
    // as if fresh.
    let mut s = session("alpha beta").at(0);
    let before = s.state.document.version();
    s.keys("dw");
    assert!(s.state.document.version() > before);
}

// ---- LSP-driven actions, without a server ----------------------------------------------
//
// The executor deliberately does not own the client: it records what to request and acts on
// diagnostics it already has. That split is what makes these testable with no process involved.

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

fn diagnostic(line: u32, start: u32, end: u32, error: bool) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: Position {
                line,
                character: start,
            },
            end: Position {
                line,
                character: end,
            },
        },
        severity: Some(if error {
            DiagnosticSeverity::ERROR
        } else {
            DiagnosticSeverity::WARNING
        }),
        message: format!("problem on line {line}"),
        ..Default::default()
    }
}

#[test]
fn hover_and_definition_record_a_request_rather_than_blocking() {
    let mut s = session("fn main() {}").at(3);

    s.keys("K");
    assert_eq!(
        s.state.pending_request,
        Some(Action::Hover),
        "the key records a request for the caller to send"
    );

    s.state.pending_request = None;
    s.keys("gd");
    assert_eq!(s.state.pending_request, Some(Action::GotoDefinition));
}

#[test]
fn diagnostic_navigation_walks_them_in_order_and_wraps() {
    let mut s = session("one\ntwo\nthree\nfour\n").at(0);
    s.state.diagnostics.apply(
        vec![diagnostic(1, 0, 3, true), diagnostic(3, 0, 4, false)],
        None,
        s.state.document.version(),
    );

    s.keys("]d");
    assert_eq!(s.cursor_line(), 1);

    s.keys("]d");
    assert_eq!(s.cursor_line(), 3);

    // Wrapping, for the same reason `n` wraps.
    s.keys("]d");
    assert_eq!(s.cursor_line(), 1);

    s.keys("[d");
    assert_eq!(
        s.cursor_line(),
        3,
        "backward from the first wraps to the last"
    );
}

#[test]
fn diagnostic_navigation_with_none_present_does_nothing() {
    let mut s = session("one\ntwo\n").at(0);
    s.keys("]d");
    assert_eq!(s.cursor(), 0, "no diagnostics means no movement");
}

#[test]
fn the_message_under_the_cursor_is_found() {
    let mut s = session("let x = 1;\nlet y = 2;\n").at(0);
    s.state.diagnostics.apply(
        vec![diagnostic(1, 4, 5, true)],
        None,
        s.state.document.version(),
    );

    // Off the diagnostic: nothing to show.
    assert!(dacode::chrome::diagnostic_at_cursor(&s.state).is_none());

    let target = s
        .state
        .document
        .text()
        .point_to_char(dacode_text::Point::new(1, 4));
    s.state.document.set_primary_caret(target);

    let (message, is_error) =
        dacode::chrome::diagnostic_at_cursor(&s.state).expect("the cursor is on the diagnostic");
    assert_eq!(message, "problem on line 1");
    assert!(is_error);
}

/// `gr` opens a rename prompt pre-filled with the symbol under the cursor.
///
/// The prompt content is the part worth pinning: an empty rename prompt means retyping the name,
/// which is how `value` becomes `vlaue`.
#[test]
fn rename_prompts_with_the_symbol_under_the_cursor() {
    let mut session = Session::new("let value = value + 1;\n");
    session.keys("w"); // onto `value`

    session.keys("gr");

    let prompt = session.state.prompt.as_ref().expect("a prompt opened");
    assert_eq!(prompt.kind, dacode::prompt::PromptKind::Rename);
    assert_eq!(prompt.input, "value", "pre-filled with the symbol");
    assert_eq!(prompt.display(), "rename to value");
}

/// Server edits arrive as one undoable change and move the selection with the text.
#[test]
fn applied_server_edits_are_a_single_undo_step_in_a_session() {
    use dacode_lsp::lsp_types::{Position, Range, TextEdit};

    let mut session = Session::new("let value = value + value;\n");
    session.keys("w");
    let version = session.state.document.version();

    let rename = |from: u32, to: u32| TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: from,
            },
            end: Position {
                line: 0,
                character: to,
            },
        },
        new_text: "total".into(),
    };

    let count = dacode::edits::apply_to_document(
        &mut session.state.document,
        &[rename(4, 9), rename(12, 17), rename(20, 25)],
        Some(version),
    )
    .expect("applies");

    assert_eq!(count, 3);
    assert_eq!(
        session.state.document.text().to_string(),
        "let total = total + total;\n"
    );

    // Undo is one press, and typing afterwards does not merge into the same group.
    session.keys("u");
    assert_eq!(
        session.state.document.text().to_string(),
        "let value = value + value;\n"
    );
}

/// With Vim keys off, typing types — through the whole editor, not just the grammar.
#[test]
fn a_non_modal_session_types_letters_instead_of_running_commands() {
    let mut session = Session::new("");
    session.grammar.set_modal(false);
    session.state.mode = Mode::Insert;
    session.state.modal = false;

    // `dw` would delete a word with the grammar on. Here it is two letters.
    session.keys("dw");
    assert_eq!(session.state.document.text().to_string(), "dw");

    // And the usual shortcuts still reach the same actions.
    session.press(Key::ctrl('a'));
    assert!(
        !session.state.document.selections().primary().is_caret(),
        "ctrl-a selects everything"
    );
    session.press(Key::ctrl('z'));
    assert_eq!(
        session.state.document.text().to_string(),
        "",
        "ctrl-z undoes"
    );
}

/// Shift with an arrow selects, which is the one gesture every editor shares.
#[test]
fn shift_arrow_extends_a_selection_without_modes() {
    let mut session = Session::new("hello world\n");
    session.grammar.set_modal(false);
    session.state.mode = Mode::Insert;

    for _ in 0..5 {
        session.press(Key::named(dacode_input::KeyCode::Right).with_shift());
    }

    let selection = session.state.document.selections().primary();
    assert!(!selection.is_caret());
    assert_eq!(
        session
            .state
            .document
            .text()
            .slice_to_string(selection.start()..selection.end()),
        "hello"
    );
}

// ---- the non-modal editor -----------------------------------------------------------------
//
// The complaint these answer: with Vim off, the editor was the modal one with the good parts
// removed. Every test below is a key a person coming from any other editor presses on their first
// day, driven through the same grammar-then-executor path the window uses.

/// A session with the modal grammar off, which is what `vim_keys = false` produces.
fn flat(contents: &str) -> Session {
    let mut session = Session::new(contents);
    session.grammar.set_modal(false);
    session.state.mode = Mode::Insert;
    session.state.modal = false;
    session
}

#[test]
fn alt_down_moves_the_line_and_takes_the_cursor_with_it() {
    // The cursor travelling with the text is the whole feature: pressing it twice has to move the
    // same line twice, not two different lines.
    let mut session = flat("first\nsecond\nthird\n").at(0);
    session.press(alt(dacode_input::KeyCode::Down));
    assert_eq!(session.text(), "second\nfirst\nthird\n");
    assert_eq!(session.cursor_line(), 1, "the cursor followed the line");

    session.press(alt(dacode_input::KeyCode::Down));
    assert_eq!(session.text(), "second\nthird\nfirst\n");
}

#[test]
fn a_line_at_the_edge_does_not_move_or_vanish() {
    let mut session = flat("only\nlines\n").at(0);
    session.press(alt(dacode_input::KeyCode::Up));
    assert_eq!(
        session.text(),
        "only\nlines\n",
        "nothing above the first line"
    );
}

#[test]
fn shift_alt_down_leaves_two_copies_and_the_cursor_on_the_lower_one() {
    let mut session = flat("line\n").at(0);
    session.press(alt(dacode_input::KeyCode::Down).with_shift());
    assert_eq!(session.text(), "line\nline\n");
    assert_eq!(session.cursor_line(), 1);
}

#[test]
fn ctrl_slash_comments_and_uncomments_the_same_lines() {
    // Round-tripping matters more than either direction alone: a toggle that leaves a stray space
    // or an extra token behind rewrites files it was only asked to look at.
    let mut session = flat("fn main() {}\n").at(0);
    session.state.file_path = Some("main.rs".into());

    session.press(Key::ctrl('/'));
    assert_eq!(session.text(), "// fn main() {}\n");

    session.press(Key::ctrl('/'));
    assert_eq!(
        session.text(),
        "fn main() {}\n",
        "back to exactly what it was"
    );
}

#[test]
fn commenting_a_block_lines_the_tokens_up_at_the_shallowest_indent() {
    let mut session = flat("if x {\n    body()\n}\n").at(0);
    session.state.file_path = Some("main.rs".into());
    // Select the whole thing, then comment it.
    session.press(Key::ctrl('a'));
    session.press(Key::ctrl('/'));

    assert_eq!(session.text(), "// if x {\n//     body()\n// }\n");
}

#[test]
fn a_file_in_a_language_with_no_comment_syntax_is_left_alone() {
    // Better to do nothing than to write `//` into a file where it means something else.
    let mut session = flat("plain text\n").at(0);
    session.press(Key::ctrl('/'));
    assert_eq!(session.text(), "plain text\n");
}

#[test]
fn ctrl_d_takes_the_word_then_the_next_copy_of_it() {
    // The first shortcut a VSCode user reaches for, and the reason it is not "add a cursor below".
    let mut session = flat("total = total + 1\n").at(0);

    session.press(Key::ctrl('d'));
    assert_eq!(session.state.document.selections().len(), 1);
    let first = *session.state.document.selections().primary();
    assert_eq!(
        session
            .state
            .document
            .text()
            .slice_to_string(first.start()..first.end()),
        "total"
    );

    session.press(Key::ctrl('d'));
    assert_eq!(
        session.state.document.selections().len(),
        2,
        "the second press adds the next occurrence as another cursor"
    );
}

#[test]
fn ctrl_shift_l_takes_every_copy_at_once() {
    let mut session = flat("a b a b a\n").at(0);
    session.press(Key::ctrl('d'));
    session.press(Key::ctrl('l').with_shift());
    assert_eq!(session.state.document.selections().len(), 3, "three a's");
}

#[test]
fn ctrl_backspace_deletes_a_word_rather_than_a_letter() {
    let mut session = flat("hello world\n").at(11);
    session.press(Key {
        code: dacode_input::KeyCode::Backspace,
        ctrl: true,
        alt: false,
        shift: false,
    });
    assert_eq!(session.text(), "hello \n");
}

#[test]
fn ctrl_shift_k_takes_the_whole_line_out() {
    let mut session = flat("keep\ndelete me\nkeep\n").at(5);
    session.press(Key::ctrl('k').with_shift());
    assert_eq!(session.text(), "keep\nkeep\n");
}

#[test]
fn a_function_key_is_not_typed_into_the_file() {
    // The bug this guards is the obvious one: F2 arriving as text and inserting "F2" — or worse, a
    // character — into the document. It must resolve to an action, and the document must not change.
    let mut session = flat("text\n").at(0);
    session.press(Key::named(dacode_input::KeyCode::Function(2)));
    assert_eq!(session.text(), "text\n");
}

/// A key with alt held, which the conventional bindings need and `Key` has no shorthand for.
fn alt(code: dacode_input::KeyCode) -> Key {
    Key {
        code,
        ctrl: false,
        alt: true,
        shift: false,
    }
}

// ---- finding things -------------------------------------------------------------------------

/// Pressing return in the search box walks the matches instead of closing it.
///
/// The complaint that produced this: the box counted matches and appeared to offer no way to visit
/// them. It did — return closed the box and `F3` stepped afterwards — but a control you have to
/// already know about, on a box that has just disappeared, is not a control anyone finds.
#[test]
fn return_in_the_search_box_walks_the_matches_and_wraps() {
    let text = "alpha\nbeta\nalpha\ngamma\nalpha\n";
    let matches = dacode_buffer::find_all(
        &dacode_text::Text::from_str(text),
        "alpha",
        &dacode_buffer::SearchOptions::default(),
    );
    assert_eq!(matches.len(), 3, "three to walk through");

    // Forward from the first lands on the second, then the third, then wraps to the first.
    let first = matches[0].start;
    let second = dacode_buffer::find_next(&matches, first, true).expect("a next match");
    assert_eq!(second.start, matches[1].start);
    let third = dacode_buffer::find_next(&matches, second.start, true).expect("a next match");
    assert_eq!(third.start, matches[2].start);
    let wrapped = dacode_buffer::find_next(&matches, third.start, true).expect("wraps");
    assert_eq!(
        wrapped.start, matches[0].start,
        "the end wraps to the start"
    );

    // And backwards, which is what shift-return does.
    let back =
        dacode_buffer::find_next(&matches, matches[1].start, false).expect("a previous match");
    assert_eq!(back.start, matches[0].start);
}

/// The position shown in the search box is the match the cursor is on, not a bare total.
#[test]
fn the_search_box_can_say_which_match_you_are_on() {
    let text = "alpha beta alpha gamma alpha";
    let matches = dacode_buffer::find_all(
        &dacode_text::Text::from_str(text),
        "alpha",
        &dacode_buffer::SearchOptions::default(),
    );

    for (index, found) in matches.iter().enumerate() {
        assert_eq!(
            dacode_buffer::index_of(&matches, found.start),
            Some(index),
            "standing on match {index} should report match {index}"
        );
    }
}

#[test]
fn escape_with_vim_off_does_not_enter_a_mode_there_is_no_way_out_of() {
    // Escape is the key every editor uses for "never mind", and with the grammar off it resolved to
    // `CollapseSelections`, which set `Mode::Normal` unconditionally. The cursor became a blue
    // block, the gutter turned blue, and the statusline went on saying EDITING — because its label
    // is chosen by `modal` rather than by the mode. A mode you could see, could not name, and could
    // not leave: nothing `feed_flat` can type is `i`.
    let mut session = flat("hello world\n").at(6);
    session.press(Key::named(dacode_input::KeyCode::Escape));

    assert_eq!(
        session.state.mode,
        Mode::Insert,
        "escape left the only mode a non-modal editor has"
    );

    // And it still has to do its job: collapsing a selection is the reason the key is bound.
    let mut session = flat("hello world\n").at(0);
    session.press(Key::ctrl('a'));
    assert!(!session.state.document.selections().primary().is_caret());
    session.press(Key::named(dacode_input::KeyCode::Escape));
    assert!(
        session.state.document.selections().primary().is_caret(),
        "escape stopped collapsing the selection"
    );
    assert_eq!(session.state.mode, Mode::Insert);
}

#[test]
fn escape_with_vim_on_still_leaves_insert_mode() {
    // The other half: refusing the mode change everywhere would break the modal editor, where
    // escape leaving insert is the single most-used key in the grammar.
    let mut session = Session::new("hello\n");
    session.state.mode = Mode::Insert;
    session.press(Key::named(dacode_input::KeyCode::Escape));
    assert_eq!(session.state.mode, Mode::Normal);
}

// ---- the syntax tree: indentation and structural text objects ------------------------------
//
// The window receives its tree from the syntax worker with each highlight result. A test has no
// worker, so `parsed` does what the worker does — parse the text and tag the tree with the version
// it saw — synchronously, and everything after it is the real key path.

impl Session {
    /// Hands the session a tree for its text as it stands, as a highlight result would.
    fn parsed(mut self, language: dacode_syntax::Language) -> Self {
        self.reparse(language);
        self
    }

    fn reparse(&mut self, language: dacode_syntax::Language) {
        let mut highlighter = dacode_syntax::Highlighter::new(language).expect("grammar loads");
        highlighter.parse(self.state.document.text());
        self.state.structure = highlighter.structure(self.state.document.version());
    }

    fn selected(&self) -> String {
        let selection = self.state.document.selections().primary();
        self.state
            .document
            .text()
            .slice_to_string(selection.start()..selection.end())
    }
}

const RUST: &str =
    "fn keep() {}\n\nfn gone(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn after() {}\n";

#[test]
fn daf_deletes_the_function_around_the_cursor() {
    let at = RUST.find("a + b").unwrap();
    let mut s = session(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("daf");
    assert_eq!(s.text(), "fn keep() {}\n\n\nfn after() {}\n");
}

#[test]
fn cif_empties_the_body_and_leaves_you_typing_in_it() {
    let at = RUST.find("a + b").unwrap();
    let mut s = session(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("cif");
    assert_eq!(s.mode(), Mode::Insert);
    assert!(
        s.text().contains("fn gone(a: i32, b: i32) -> i32 {\n}\n"),
        "the braces stay where they were: {:?}",
        s.text()
    );
}

#[test]
fn cia_changes_one_argument() {
    let at = RUST.find("b: i32").unwrap();
    let mut s = session(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("ciac:u8");
    assert!(
        s.text().contains("fn gone(a: i32, c:u8) -> i32"),
        "{:?}",
        s.text()
    );
}

#[test]
fn daa_takes_the_comma_with_the_argument() {
    let at = RUST.find("a: i32").unwrap();
    let mut s = session(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("daa");
    assert!(
        s.text().contains("fn gone(b: i32) -> i32"),
        "{:?}",
        s.text()
    );
}

#[test]
fn yat_yanks_the_type_and_leaves_the_text_alone() {
    let source = "struct Point {\n    x: i32,\n}\n";
    let mut s = session(source).at(19).parsed(dacode_syntax::Language::Rust);
    s.keys("yat");
    assert_eq!(s.text(), source);
    assert_eq!(s.registers.get(None), source);
}

#[test]
fn vaa_selects_the_argument_in_visual_mode() {
    let at = RUST.find("b: i32").unwrap();
    let mut s = session(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("vaa");
    assert_eq!(s.mode(), Mode::Visual);
    // Visual mode draws the head's character as selected, so the selection's own range stops one
    // short of what `d` would take.
    s.keys("d");
    assert!(
        s.text().contains("fn gone(a: i32) -> i32"),
        "{:?}",
        s.text()
    );
}

#[test]
fn dic_deletes_the_words_of_a_python_comment() {
    let source = "def f(x):\n    # say what it is for\n    return x\n";
    let at = source.find("what").unwrap();
    let mut s = session(source)
        .at(at)
        .parsed(dacode_syntax::Language::Python);
    s.keys("dic");
    assert_eq!(s.text(), "def f(x):\n    # \n    return x\n");
}

#[test]
fn daf_in_python_takes_the_whole_definition() {
    let source = "def f(x):\n    y = x\n    return y\n\nz = 1\n";
    let at = source.find("y = x").unwrap();
    let mut s = session(source)
        .at(at)
        .parsed(dacode_syntax::Language::Python);
    s.keys("daf");
    assert_eq!(s.text(), "\nz = 1\n");
}

#[test]
fn a_stale_tree_selects_nothing_and_says_so() {
    // One keystroke after the parse the tree's offsets name text that has moved. `daf` must not
    // delete whatever range used to be the function: it does nothing, and says why.
    let at = RUST.find("a + b").unwrap();
    let mut s = session(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("x");
    let after_edit = s.text();
    s.keys("daf");
    assert_eq!(s.text(), after_edit, "nothing deleted");
    let message = s.state.hover.clone().unwrap_or_default();
    assert!(message.contains("catching up"), "{message:?}");

    // And once the tree has caught up, the same keys work.
    s.reparse(dacode_syntax::Language::Rust);
    s.keys("daf");
    assert_eq!(s.text(), "fn keep() {}\n\n\nfn after() {}\n");
}

#[test]
fn no_function_there_is_a_message_not_a_guess() {
    let mut s = session("use std::fmt;\n\nfn f() {}\n")
        .at(5)
        .parsed(dacode_syntax::Language::Rust);
    s.keys("daf");
    assert_eq!(s.text(), "use std::fmt;\n\nfn f() {}\n");
    assert_eq!(s.state.hover.as_deref(), Some("no function here"));
}

#[test]
fn enter_indents_from_the_tree_when_it_is_fresh() {
    // The lexical rule copies this line's sixteen columns of alignment; the tree knows the call has
    // closed and the statement has ended, so the next line belongs to the block.
    let source = "fn main() {\n    let x = foo(a,\n                b);\n}\n";
    let at = source.find("b);").unwrap() + 3;
    let mut s = flat(source).at(at).parsed(dacode_syntax::Language::Rust);
    s.press(Key::named(dacode_input::KeyCode::Enter));
    assert!(
        s.text().contains("                b);\n    \n}"),
        "{:?}",
        s.text()
    );
}

#[test]
fn enter_falls_back_to_the_lexical_rule_when_the_tree_is_stale() {
    // Typed a character since the parse: the tree is a version behind and is not asked.
    let source = "fn main() {\n    let x = foo(a,\n                b);\n}\n";
    let at = source.find("b);").unwrap() + 3;
    let mut s = flat(source).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("z");
    s.press(Key::named(dacode_input::KeyCode::Enter));
    assert!(
        s.text().contains("b);z\n                \n}"),
        "{:?}",
        s.text()
    );
}

#[test]
fn the_non_modal_keys_select_function_type_and_argument() {
    let at = RUST.find("b: i32").unwrap();
    let mut s = flat(RUST).at(at).parsed(dacode_syntax::Language::Rust);
    s.press(alt(dacode_input::KeyCode::Char('a')));
    assert_eq!(s.selected(), "b: i32");
    s.press(alt(dacode_input::KeyCode::Char('f')));
    assert_eq!(
        s.selected(),
        "fn gone(a: i32, b: i32) -> i32 {\n    a + b\n}\n"
    );

    let source = "impl A {\n    fn f() {}\n}\n";
    let mut s = flat(source).at(18).parsed(dacode_syntax::Language::Rust);
    s.press(alt(dacode_input::KeyCode::Char('t')));
    assert_eq!(s.selected(), source);
}

#[test]
fn shift_alt_right_grows_the_selection_one_node_at_a_time() {
    let source = "fn f() { g(a + b); }\n";
    let at = source.find("a + b").unwrap();
    let mut s = flat(source).at(at).parsed(dacode_syntax::Language::Rust);
    let grow = alt(dacode_input::KeyCode::Right).with_shift();
    s.press(grow);
    assert_eq!(s.selected(), "a");
    s.press(grow);
    assert_eq!(s.selected(), "a + b");
    s.press(grow);
    assert_eq!(s.selected(), "(a + b)");
}

// ---- ctrl-w and ctrl-t, through the grammar to the window ------------------------------------
//
// Closing a buffer and opening the terminal are the event loop's — the executor hands them back as
// `Outcome::Window` — so this is as far as a headless session goes. The binary's own tests carry
// the same keys on through `App::press` to a buffer actually closing and a shell actually starting.

impl Session {
    /// The window action one key resolves to, through the grammar and the executor.
    fn window_action(&mut self, keys: &str) -> Option<Action> {
        let mut last = None;
        for key in Key::parse_sequence(keys) {
            if let Resolution::Action(action) = self.grammar.feed(self.state.mode, key) {
                if let Outcome::Window(window) =
                    execute(&mut self.state, &mut self.registers, &action, 40)
                {
                    last = Some(window);
                }
            }
        }
        last
    }
}

#[test]
fn ctrl_w_closes_the_file_and_ctrl_t_toggles_the_terminal_from_any_mode() {
    // Insert mode included: Vim's own ctrl-w deletes a word there, and leaving it to do that while
    // every other mode closed the file would make the key mean whatever mode you forgot you were in.
    for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
        let mut s = session("let x = 1;\n");
        s.state.set_mode(mode);
        assert_eq!(
            s.window_action("<c-w>"),
            Some(Action::CloseBuffer),
            "ctrl-w in {mode:?}"
        );
        assert_eq!(s.text(), "let x = 1;\n", "and nothing typed into the file");
        assert_eq!(
            s.window_action("<c-t>"),
            Some(Action::ToggleTerminal),
            "ctrl-t in {mode:?}"
        );
    }
    // The window commands are still there, after the prefix they moved to.
    let mut s = session("x\n");
    assert_eq!(
        s.window_action("<c-k>v"),
        Some(Action::SplitPane { vertical: true })
    );
}

// ---- counted structural objects, and shrink-selection ----------------------------------------

const NESTED: &str = "fn outer() {\n    let f = |x| {\n        x + 1\n    };\n}\n\nfn after() {}\n";

#[test]
fn two_daf_deletes_the_function_around_the_closure() {
    // `daf` inside a closure takes the closure; `2daf` and `d2af` take the function it is in.
    let at = NESTED.find("x + 1").unwrap();
    for keys in ["2daf", "d2af"] {
        let mut s = session(NESTED).at(at).parsed(dacode_syntax::Language::Rust);
        s.keys(keys);
        assert_eq!(s.text(), "\nfn after() {}\n", "{keys}");
    }
    let mut s = session(NESTED).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("daf");
    assert!(
        s.text().starts_with("fn outer() {\n    let f = ;"),
        "{:?}",
        s.text()
    );
}

#[test]
fn a_count_past_the_outermost_deletes_nothing_and_says_so() {
    // `3daf` with only two functions around must not fall back to the outermost: the user asked
    // for something that is not there, and deleting something else would be the worst answer.
    let at = NESTED.find("x + 1").unwrap();
    let mut s = session(NESTED).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("3daf");
    assert_eq!(s.text(), NESTED);
    assert_eq!(s.state.hover.as_deref(), Some("no function here"));
}

#[test]
fn v2aa_selects_the_argument_one_list_out() {
    let source = "fn f() { g(a, h(b, c)); }\n";
    let at = source.find("b, c").unwrap();
    let mut s = session(source).at(at).parsed(dacode_syntax::Language::Rust);
    s.keys("v2ia");
    s.keys("d");
    assert_eq!(s.text(), "fn f() { g(a, ); }\n");
}

/// A session with the modal grammar off, where shift-alt-arrows are expand and shrink.
fn flat_session(source: &str, at: usize) -> Session {
    let mut s = session(source).at(at).parsed(dacode_syntax::Language::Rust);
    s.grammar.set_modal(false);
    s.state.modal = false;
    s.state.mode = Mode::Insert;
    s
}

#[test]
fn shrinking_retraces_each_expansion_back_to_the_caret() {
    // Expand three times and shrink three times and you are where you started — including the
    // caret's place inside the word, which no node of the tree records: only the history can say.
    let source = "fn f() { g(alpha + b); }\n";
    let at = source.find("lpha").unwrap();
    let mut s = flat_session(source, at);
    s.keys("<s-a-→>");
    assert_eq!(s.selected(), "alpha");
    s.keys("<s-a-→><s-a-→>");
    assert_eq!(s.selected(), "(alpha + b)");
    s.keys("<s-a-←>");
    assert_eq!(s.selected(), "alpha + b");
    s.keys("<s-a-←><s-a-←>");
    assert_eq!(s.selected(), "");
    assert_eq!(s.cursor(), at, "back on the character it started on");
}

#[test]
fn with_no_history_shrinking_asks_the_tree() {
    // A selection made some other way — here `select_argument` — has no expansions behind it, so
    // shrinking steps to the named node under the caret inside it.
    let source = "fn f() { g(alpha + b); }\n";
    let at = source.find("alpha").unwrap();
    let mut s = flat_session(source, at);
    s.keys("<a-a>");
    assert_eq!(s.selected(), "alpha + b");
    s.keys("<s-a-←>");
    assert_eq!(
        s.selected(),
        "b",
        "the head sits at the end, so the child there"
    );
}

#[test]
fn an_edit_ends_the_expansion_history() {
    // The history holds offsets, and `mutate` clears it rather than moving it. An edit after the
    // selection leaves the selection's offsets exactly where they were, so this is the one case the
    // "are the selections still what expand left?" check cannot catch on its own: without the
    // clear, shrinking would put the caret back from a history of a document that no longer exists.
    let source = "fn f() { g(alpha + b); }\n";
    let at = source.find("lpha").unwrap();
    let mut s = flat_session(source, at);
    s.keys("<s-a-→>");
    assert_eq!(s.selected(), "alpha");
    let end = s.state.document.text().len_chars();
    s.state.mutate(|document| {
        document
            .apply(
                dacode_text::Transaction::new(vec![dacode_text::Edit::insert(end, "// x\n")]),
                dacode_buffer::EditKind::Discrete,
            )
            .unwrap()
    });
    s.reparse(dacode_syntax::Language::Rust);
    assert_eq!(s.selected(), "alpha", "the edit was after the selection");
    s.keys("<s-a-←>");
    assert_eq!(
        s.selected(),
        "alpha",
        "the tree has nothing inside an identifier, so the selection stays — not the old caret"
    );
}

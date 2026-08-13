//! Executing [`Action`]s against the editor state.
//!
//! The other half of the `Action` decision: everything the editor does arrives here as one
//! value, so there is exactly one place where an operation can be wrong, and dot-repeat and
//! macros are replay rather than re-implementation.
//!
//! This is the seam where `crates/workspace` will eventually live. It sits in the binary for now
//! because it is the only consumer.

use crate::chrome::EditorState;
use crate::prompt::{Prompt, PromptKind};
use crate::results::Results;
use vex_buffer::EditKind;
use vex_input::{
    Action, InsertPosition, Motion, Operator, Registers, Target, line_range, operator_range,
    resolve_motion, resolve_object,
};
use vex_text::{CharOffset, Edit, Point, Selection, SelectionSet, Transaction};
use vex_theme::Mode;

pub enum Outcome {
    Continue,
    Quit,
    /// A window-management action the caller has to carry out, because it needs the pane and buffer
    /// lists that the executor deliberately does not have.
    Window(Action),
}

/// Applies one action. `rows` is the viewport height, needed for display-aware vertical motion.
pub fn execute(
    state: &mut EditorState,
    registers: &mut Registers,
    action: &Action,
    rows: usize,
) -> Outcome {
    match action {
        Action::Move { motion, count } => move_selections(state, *motion, *count, false),
        Action::Extend { motion, count } => move_selections(state, *motion, *count, true),

        Action::Apply {
            operator,
            target,
            register,
        } => apply_operator(state, registers, *operator, *target, *register),

        Action::EnterMode(mode) => {
            state.mode = *mode;
            state.document.end_undo_group();
        }
        Action::InsertAt(position) => enter_insert(state, *position),

        Action::InsertText(text) => {
            // Typed characters go through the pairing rules; longer runs (a paste, a snippet) do
            // not, because pairing a whole string would mangle it.
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(single), None) => {
                    let config = state.pairs.clone();
                    state.document.type_char(single, &config);
                }
                _ => state.document.insert(text),
            }
        }
        Action::Newline => {
            let config = state.pairs.clone();
            state.document.insert_newline_smart(&config);
        }
        Action::DeleteBackward => {
            let config = state.pairs.clone();
            state.document.delete_backward_pairs(&config);
        }
        Action::DeleteForward => state.document.delete_forward(),
        Action::ReplaceChar(c) => {
            // Overwrite: one intent, so the delete and insert land in one undo group.
            state.document.delete_forward();
            state.document.insert(&c.to_string());
        }

        Action::Paste {
            register,
            before,
            count,
        } => paste(state, registers, *register, *before, *count),

        Action::Undo => {
            state.document.undo();
        }
        Action::Redo => {
            state.document.redo();
        }
        // Handled by the caller, which owns the last-change record.
        Action::RepeatChange => {}

        Action::AddCaretBelow => state.document.add_caret_vertically(1),
        Action::AddCaretAbove => state.document.add_caret_vertically(-1),
        Action::SplitSelectionIntoLines => split_into_lines(state),
        Action::KeepPrimarySelection => {
            let primary = *state.document.selections().primary();
            state.document.set_selections(SelectionSet::single(primary));
        }
        Action::CollapseSelections => {
            state.document.collapse_selections();
            state.mode = Mode::Normal;
        }
        Action::SelectLine { count } => select_lines(state, *count),
        Action::SelectAll => {
            let end = state.document.text().len_chars();
            state
                .document
                .set_selections(SelectionSet::single(Selection::new(CharOffset::ZERO, end)));
        }

        Action::ToggleSoftWrap => {
            state.display.mode = match state.display.mode {
                vex_display::WrapMode::Word => vex_display::WrapMode::None,
                vex_display::WrapMode::None => vex_display::WrapMode::Word,
            };
        }
        Action::ToggleTheme => {
            state.theme = vex_theme::Theme::for_appearance(match state.theme.appearance {
                vex_theme::Appearance::Dark => vex_theme::Appearance::Light,
                vex_theme::Appearance::Light => vex_theme::Appearance::Dark,
            });
        }
        Action::IncreaseFontSize => {
            state.typography =
                vex_theme::Typography::with_code_size(state.typography.code_size + 1.0);
        }
        Action::DecreaseFontSize => {
            state.typography =
                vex_theme::Typography::with_code_size(state.typography.code_size - 1.0);
        }

        Action::Save => save(state),
        Action::Quit => return Outcome::Quit,

        Action::StartSearch { forward } => {
            state.prompt = Some(Prompt::new(
                PromptKind::Search { forward: *forward },
                state.cursor(),
            ));
        }
        Action::StartCommand => {
            state.prompt = Some(Prompt::new(PromptKind::Command, state.cursor()));
        }
        Action::RepeatSearch { forward } => repeat_search(state, *forward),
        // Requests go out through the caller, which owns the client; the executor records what was
        // asked for. Splitting it this way keeps `execute` free of process handling and testable
        // without a language server.
        Action::Hover | Action::GotoDefinition | Action::Format => {
            state.pending_request = Some(action.clone())
        }
        Action::Rename => {
            // Pre-filled with the symbol under the cursor, because a rename is nearly always an edit
            // of the existing name rather than a fresh one.
            let mut prompt = Prompt::new(PromptKind::Rename, state.cursor());
            prompt.input = word_under_cursor(state);
            state.prompt = Some(prompt);
        }
        Action::RequestCompletion => {
            // The session opens immediately, marked pending, so the popup can say it is waiting
            // rather than appearing out of nowhere when the reply lands.
            state.completion = Some(crate::completion::Completion::new(
                state.document.text(),
                state.cursor(),
                state.document.version(),
            ));
            state.pending_request = Some(action.clone());
        }
        Action::NextDiagnostic { forward } => next_diagnostic(state, *forward),
        Action::DismissInfo => state.hover = None,

        // Window management belongs to the caller: the executor has no idea how many panes exist,
        // and giving it one would mean giving it the buffer list too.
        Action::SplitPane { .. } | Action::ClosePane | Action::FocusPane { .. } => {
            return Outcome::Window(action.clone());
        }

        Action::SearchProject => {
            // The word under the cursor is the query people actually want from a bare invocation.
            let query = word_under_cursor(state);
            if query.is_empty() {
                state.prompt = Some(Prompt::new(PromptKind::Command, state.cursor()));
                if let Some(prompt) = &mut state.prompt {
                    prompt.input.push_str("grep ");
                }
            } else {
                state.results = Some(Results::start(&state.project_root, &query));
            }
        }
    }

    // Vertical motion is the only action that needs the viewport; everything else is independent
    // of it. Kept as an argument rather than state so the executor stays testable.
    let _ = rows;
    Outcome::Continue
}

/// Resolves a motion for every selection. `Up`/`Down` go through the display map so they follow
/// soft wrap; everything else is a text query.
fn move_selections(state: &mut EditorState, motion: Motion, count: usize, extend: bool) {
    let text = state.document.text().clone();
    let display = state.display.clone();

    let selections = state.document.selections().map(|selection| {
        let head = match motion {
            Motion::Up | Motion::Down => {
                let delta = if motion == Motion::Down { 1 } else { -1 };
                let mut at = selection.head;
                for _ in 0..count.max(1) {
                    at = display.move_vertically(&text, at, delta, None);
                }
                at
            }
            _ => resolve_motion(&text, selection.head, motion, count),
        };

        if extend {
            selection.extend_to(head)
        } else {
            Selection::caret(head)
        }
    });

    state.document.set_selections(selections);
}

/// The ranges an operator acts on, one per selection: sorted, disjoint and in bounds, so they
/// form a valid transaction.
fn operator_ranges(state: &EditorState, target: Target) -> Vec<std::ops::Range<CharOffset>> {
    let text = state.document.text();
    let mut ranges: Vec<std::ops::Range<CharOffset>> = Vec::new();

    for selection in state.document.selections().iter() {
        let range = match target {
            Target::Selection => {
                // Visual mode: the selection is inclusive of the character under the head.
                let end = text.next_grapheme(selection.end());
                Some(selection.start()..end)
            }
            Target::Motion { motion, count } => {
                let to = match motion {
                    Motion::Up | Motion::Down => {
                        let delta = if motion == Motion::Down { 1 } else { -1 };
                        let line = text
                            .char_to_line(selection.head)
                            .saturating_add_signed(delta * count.max(1) as isize);
                        text.point_to_char(Point::new(line.min(text.last_line()), 0))
                    }
                    _ => resolve_motion(text, selection.head, motion, count),
                };
                let range = operator_range(text, selection.head, to, motion);
                (range.start < range.end).then_some(range)
            }
            Target::WholeLine { count } => {
                let line = text.char_to_line(selection.head);
                let last = (line + count.max(1) - 1).min(text.last_line());
                Some(line_range(text, line, last))
            }
            Target::Object(object) => resolve_object(text, selection.head, object),
        };

        if let Some(range) = range {
            ranges.push(range);
        }
    }

    // Multiple cursors can produce overlapping ranges (two cursors in the same word, both doing
    // `diw`). Clip rather than reject, so the keystroke does something sensible.
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut clipped: Vec<std::ops::Range<CharOffset>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match clipped.last() {
            Some(previous) if range.start < previous.end => {
                if range.end > previous.end {
                    let start = previous.end;
                    clipped.push(start..range.end);
                }
            }
            _ => clipped.push(range),
        }
    }
    clipped
}

fn apply_operator(
    state: &mut EditorState,
    registers: &mut Registers,
    operator: Operator,
    target: Target,
    register: Option<char>,
) {
    let ranges = operator_ranges(state, target);
    if ranges.is_empty() {
        return;
    }

    // Acting on a visual selection consumes it: the selection was the operand, so staying in
    // visual mode would leave a stale range highlighted.
    let leaving_visual = state.mode == Mode::Visual;

    let text = state.document.text().clone();
    let contents: Vec<String> = ranges
        .iter()
        .map(|range| text.slice_to_string(range.clone()))
        .collect();

    match operator {
        Operator::Yank => {
            registers.set(register, contents.join(""));
            if leaving_visual {
                state.mode = Mode::Normal;
            }
            // Yanking leaves the text alone, so collapse to the start of each range the way Vim
            // leaves the cursor at the top of a yank.
            let heads: Vec<Selection> = ranges
                .iter()
                .map(|range| Selection::caret(range.start))
                .collect();
            state
                .document
                .set_selections(SelectionSet::from_iter(heads, 0));
        }

        Operator::Delete | Operator::Change => {
            registers.set(register, contents.join(""));
            let edits = ranges
                .iter()
                .map(|range| Edit::delete(range.clone()))
                .collect();
            let _ = state
                .document
                .apply(Transaction::new(edits), EditKind::Discrete);

            if operator == Operator::Change {
                state.mode = Mode::Insert;
            } else if leaving_visual {
                state.mode = Mode::Normal;
            }
        }

        Operator::Uppercase | Operator::Lowercase => {
            let edits = ranges
                .iter()
                .zip(&contents)
                .map(|(range, content)| {
                    let replacement = if operator == Operator::Uppercase {
                        content.to_uppercase()
                    } else {
                        content.to_lowercase()
                    };
                    Edit::replace(range.clone(), replacement)
                })
                .collect();
            let _ = state
                .document
                .apply(Transaction::new(edits), EditKind::Discrete);
            if leaving_visual {
                state.mode = Mode::Normal;
            }
        }

        Operator::Indent | Operator::Outdent => {
            indent(state, &ranges, operator == Operator::Indent)
        }
    }
}

/// Indents or outdents whole lines. Line-based, because indenting half a line is never what was
/// meant.
fn indent(state: &mut EditorState, ranges: &[std::ops::Range<CharOffset>], forward: bool) {
    let text = state.document.text().clone();
    let mut lines: Vec<usize> = Vec::new();

    for range in ranges {
        let first = text.char_to_line(range.start);
        let last =
            text.char_to_line(CharOffset(range.end.get().saturating_sub(1)).max(range.start));
        for line in first..=last {
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    lines.sort_unstable();

    const WIDTH: usize = 4;
    let mut edits = Vec::new();

    for line in lines {
        let start = text.line_to_char(line);
        if forward {
            edits.push(Edit::insert(start, " ".repeat(WIDTH)));
        } else {
            let length = text.line_len(line);
            let content = text.slice_to_string(start..CharOffset(start.get() + length));
            let removable = content
                .chars()
                .take(WIDTH)
                .take_while(|c| *c == ' ')
                .count();
            if removable > 0 {
                edits.push(Edit::delete(start..CharOffset(start.get() + removable)));
            }
        }
    }

    if !edits.is_empty() {
        let _ = state
            .document
            .apply(Transaction::new(edits), EditKind::Discrete);
    }
}

fn enter_insert(state: &mut EditorState, position: InsertPosition) {
    let text = state.document.text().clone();

    match position {
        InsertPosition::BeforeCursor => {}
        InsertPosition::AfterCursor => {
            let selections = state
                .document
                .selections()
                .map(|selection| Selection::caret(text.next_grapheme(selection.head)));
            state.document.set_selections(selections);
        }
        InsertPosition::LineStart => state.document.move_to_line_edge(false, false),
        InsertPosition::LineEnd => state.document.move_to_line_edge(true, false),
        InsertPosition::LineBelow => {
            state.document.move_to_line_edge(true, false);
            state.document.insert_newline();
        }
        InsertPosition::LineAbove => {
            state.document.move_to_line_edge(false, false);
            state.document.insert("\n");
            state.document.move_grapheme(false, false);
        }
    }

    state.mode = Mode::Insert;
    state.document.end_undo_group();
}

fn paste(
    state: &mut EditorState,
    registers: &mut Registers,
    register: Option<char>,
    before: bool,
    count: usize,
) {
    let contents = registers.get(register).to_string();
    if contents.is_empty() {
        return;
    }

    let linewise = registers.is_linewise(register);
    let repeated = contents.repeat(count.max(1));
    let text = state.document.text().clone();

    let edits: Vec<Edit> = state
        .document
        .selections()
        .iter()
        .map(|selection| {
            let at = if linewise {
                // Linewise pastes land on whole-line boundaries, not mid-line.
                let line = text.char_to_line(selection.head);
                if before {
                    text.line_to_char(line)
                } else {
                    let next = (line + 1).min(text.last_line());
                    if next == line {
                        text.len_chars()
                    } else {
                        text.line_to_char(next)
                    }
                }
            } else if before {
                selection.start()
            } else {
                text.next_grapheme(selection.head)
            };
            Edit::insert(at, repeated.clone())
        })
        .collect();

    let _ = state
        .document
        .apply(Transaction::new(edits), EditKind::Discrete);
}

fn split_into_lines(state: &mut EditorState) {
    let text = state.document.text().clone();
    let mut carets = Vec::new();

    for selection in state.document.selections().iter() {
        let first = text.char_to_line(selection.start());
        let last = text.char_to_line(selection.end());
        for line in first..=last {
            carets.push(Selection::caret(text.point_to_char(Point::new(line, 0))));
        }
    }

    if !carets.is_empty() {
        let primary = carets.len() - 1;
        state
            .document
            .set_selections(SelectionSet::from_iter(carets, primary));
    }
}

fn select_lines(state: &mut EditorState, count: usize) {
    let text = state.document.text().clone();
    let selections = state.document.selections().map(|selection| {
        let line = text.char_to_line(selection.head);
        let last = (line + count.max(1) - 1).min(text.last_line());
        let range = line_range(&text, line, last);
        Selection::new(range.start, range.end)
    });
    state.document.set_selections(selections);
}

/// Moves to the next or previous match of the last search.
///
/// Uses the cached match list rather than re-scanning: `n` is pressed repeatedly, and rescanning the
/// document on each press would make it feel heavier the larger the file.
fn repeat_search(state: &mut EditorState, forward: bool) {
    if state.search_matches.is_empty() {
        return;
    }
    let from = state.cursor();
    if let Some(found) = vex_buffer::find_next(&state.search_matches, from, forward) {
        state.document.set_primary_caret(found.start);
    }
}

/// The word the cursor sits in or just after, for a bare project search.
fn word_under_cursor(state: &EditorState) -> String {
    let text = state.document.text();
    let object = vex_input::TextObject::Word {
        around: false,
        big: false,
    };
    match vex_input::resolve_object(text, state.cursor(), object) {
        Some(range) if range.start < range.end => text.slice_to_string(range),
        _ => String::new(),
    }
}

/// Moves the cursor to the next or previous diagnostic in this file.
fn next_diagnostic(state: &mut EditorState, forward: bool) {
    let text = state.document.text().clone();
    let mut starts: Vec<CharOffset> = state
        .diagnostics
        .items()
        .iter()
        .map(|diagnostic| vex_lsp::range_from_lsp(&text, diagnostic.range).start)
        .collect();
    if starts.is_empty() {
        return;
    }
    starts.sort_unstable();
    starts.dedup();

    let cursor = state.cursor();
    // Wrapping, for the same reason `n` wraps: stopping at the last one forces a manual jump back.
    let target = if forward {
        starts
            .iter()
            .find(|start| **start > cursor)
            .copied()
            .or_else(|| starts.first().copied())
    } else {
        starts
            .iter()
            .rev()
            .find(|start| **start < cursor)
            .copied()
            .or_else(|| starts.last().copied())
    };

    if let Some(target) = target {
        state.document.set_primary_caret(target);
        // The message for the diagnostic just jumped to is the point of jumping there.
        state.hover = None;
    }
}

fn save(state: &mut EditorState) {
    let Some(path) = state.file_path.clone() else {
        eprintln!("vex: no file to save to");
        return;
    };

    match std::fs::write(&path, state.document.text().to_string()) {
        Ok(()) => {
            state.document.mark_saved();
            eprintln!("vex: wrote {path}");
        }
        // Reported, never silent: a save that failed while the user believes it succeeded is
        // the worst failure an editor has.
        Err(err) => eprintln!("vex: could not write {path}: {err}"),
    }
}

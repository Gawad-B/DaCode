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
use dacode_buffer::EditKind;
use dacode_input::{
    Action, InsertPosition, Motion, Operator, Registers, Target, line_range, operator_range,
    resolve_motion, resolve_object,
};
use dacode_text::{CharOffset, Edit, Point, Selection, SelectionSet, Transaction};
use dacode_theme::Mode;

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
            state.set_mode(*mode);
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
                    state.mutate(|document| document.type_char(single, &config));
                }
                _ => state.mutate(|document| document.insert(text)),
            }
        }
        Action::Newline => {
            let config = state.pairs.clone();
            state.mutate(|document| document.insert_newline_smart(&config));
        }
        Action::DeleteBackward => {
            let config = state.pairs.clone();
            state.mutate(|document| document.delete_backward_pairs(&config));
        }
        Action::DeleteForward => state.mutate(|document| document.delete_forward()),
        Action::ReplaceChar(c) => {
            // Overwrite: one intent, so the delete and insert land in one undo group.
            state.mutate(|document| {
                document.delete_forward();
                document.insert(&c.to_string());
            });
        }

        Action::Paste {
            register,
            before,
            count,
        } => paste(state, registers, *register, *before, *count),

        Action::Undo => {
            state.mutate(|document| document.undo());
        }
        Action::Redo => {
            state.mutate(|document| document.redo());
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
            state.set_mode(Mode::Normal);
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
                dacode_display::WrapMode::Word => dacode_display::WrapMode::None,
                dacode_display::WrapMode::None => dacode_display::WrapMode::Word,
            };
        }
        Action::ToggleTheme => {
            state.theme = dacode_theme::Theme::for_appearance(match state.theme.appearance {
                dacode_theme::Appearance::Dark => dacode_theme::Appearance::Light,
                dacode_theme::Appearance::Light => dacode_theme::Appearance::Dark,
            });
        }
        Action::IncreaseFontSize => {
            state.typography =
                dacode_theme::Typography::with_code_size(state.typography.code_size + 1.0);
        }
        Action::DecreaseFontSize => {
            state.typography =
                dacode_theme::Typography::with_code_size(state.typography.code_size - 1.0);
        }

        // The app saves, not the executor: a save also clears the recovery copy, tells the watcher
        // the write was its own, checks nobody else changed the file, and may start a format. This
        // arm wrote the file and did none of that, and it was the one `ctrl-s` reached.
        Action::Save => return Outcome::Window(action.clone()),
        Action::SaveAs => {
            // Pre-filled with where the file already is, so "save a copy beside it" is an edit of
            // the name rather than typing a path out.
            let mut prompt = Prompt::new(PromptKind::SaveAs, state.cursor());
            prompt.input = state.file_path.clone().unwrap_or_default();
            state.prompt = Some(prompt);
        }
        Action::ToggleAutosave => {
            state.autosave = !state.autosave;
            state.hover = Some(match state.autosave {
                true => "autosave on — the file is written a moment after you stop typing".into(),
                false => "autosave off — ctrl-s writes the file".into(),
            });
        }
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
        Action::Hover
        | Action::GotoDefinition
        | Action::Format
        | Action::CodeAction
        | Action::FindReferences
        | Action::GotoSymbol
        | Action::GotoSymbolInProject => state.pending_request = Some(action.clone()),

        // Walking the diff needs nothing but the marks already on screen, so it happens here.
        Action::GotoChange { forward } => {
            let line = state.cursor_point().line;
            match state.diff.change_near(line, *forward) {
                Some(target) => {
                    let text = state.document.text();
                    let at = text
                        .point_to_char(dacode_text::Point::new(target.min(text.last_line()), 0));
                    state.document.set_primary_caret(at);
                }
                None => state.hover = Some("nothing has changed since the last commit".into()),
            }
        }
        // Reverting needs git, which the event loop owns.
        Action::RevertHunk => return Outcome::Window(action.clone()),
        // The jumplist and macros need the editor's own history, which the executor does not hold —
        // the same split as the window actions.
        Action::OpenFolder
        | Action::ToggleSettings
        | Action::ExpandSnippet
        | Action::Jump { .. }
        | Action::RecordMacro { .. }
        | Action::ReplayMacro { .. } => {
            return Outcome::Window(action.clone());
        }

        Action::ToggleFold => {
            let line = state.cursor_point().line;
            if state.display.folds.open(line) {
                // Already folded here: opening it is the toggle.
            } else if let Some(range) =
                dacode_display::fold_range(state.document.text(), line, state.display.tab_width)
            {
                state.display.folds.close(range);
            } else {
                // Not a block head. Fold the block this line is *inside*, which is what someone
                // pressing `za` in the middle of a function means by it.
                let mut candidate = line;
                while candidate > 0 {
                    candidate -= 1;
                    if let Some(range) = dacode_display::fold_range(
                        state.document.text(),
                        candidate,
                        state.display.tab_width,
                    ) {
                        if range.end >= line {
                            state.display.folds.close(range);
                            // The cursor cannot stay on a hidden line.
                            let head = state
                                .document
                                .text()
                                .point_to_char(Point::new(candidate, 0));
                            state.document.set_primary_caret(head);
                            break;
                        }
                    }
                }
            }
        }
        Action::FoldAll { close } => {
            state.display.folds.clear();
            if *close {
                let text = state.document.text();
                let mut line = 0;
                while line <= text.last_line() {
                    match dacode_display::fold_range(text, line, state.display.tab_width) {
                        // Top-level blocks only: closing nested ones too would hide nothing extra,
                        // since the outer fold already covers them.
                        Some(range) => {
                            let end = range.end;
                            state.display.folds.close(range);
                            line = end + 1;
                        }
                        None => line += 1,
                    }
                }

                let line = state.cursor_point().line;
                if state.display.folds.is_hidden(line) {
                    // Land on the head of whatever now hides the cursor.
                    let mut head = line;
                    while head > 0 && state.display.folds.is_hidden(head) {
                        head -= 1;
                    }
                    let offset = state.document.text().point_to_char(Point::new(head, 0));
                    state.document.set_primary_caret(offset);
                }
            }
        }
        Action::ToggleDiagnosticsList => {
            state.show_diagnostics = !state.show_diagnostics;
            // The tree and the diagnostics list share the sidebar; showing one hides the other
            // rather than one silently winning.
            if state.show_diagnostics {
                state.tree = None;
            }
        }
        Action::ShowSourceControl | Action::ShowRunAndDebug => {
            let view = if *action == Action::ShowSourceControl {
                crate::layout::Activity::SourceControl
            } else {
                crate::layout::Activity::Debug
            };
            // A second press closes it, as the activity icons do in every editor that has them.
            if state.chrome.activity == Some(view) {
                state.chrome.activity = None;
            } else {
                state.close_sidebar();
                state.chrome.activity = Some(view);
            }
        }
        Action::ToggleNotifications => {
            state.chrome.show_notifications = !state.chrome.show_notifications;
            state.chrome.unread = 0;
        }
        // Both need the machine: the clipboard, and the files a link is read from.
        Action::Share | Action::Run => return Outcome::Window(action.clone()),
        Action::ToggleInlayHints => {
            state.show_inlay_hints = !state.show_inlay_hints;
            state.inlay_hints.clear();
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
        // The window action list owns this: the clamp needs the rows on screen, which the caller
        // has and the executor's `rows` argument gives it.
        Action::ScrollHorizontally { right } => {
            if state.display.mode == dacode_display::WrapMode::None {
                const STEP: usize = 8;
                let visible =
                    state
                        .display
                        .rows_from(state.document.text(), state.scroll, rows.max(1));
                let longest = visible.iter().map(|row| row.len()).max().unwrap_or(0);
                state.scroll.column = if *right {
                    (state.scroll.column + STEP).min(longest.saturating_add(STEP))
                } else {
                    state.scroll.column.saturating_sub(STEP)
                };
            }
        }
        Action::StartReplace => {
            state.prompt = Some(Prompt::new(PromptKind::Replace, state.cursor()));
            state.search_matches.clear();
        }
        Action::NextDiagnostic { forward } => next_diagnostic(state, *forward),
        Action::DismissInfo => state.hover = None,

        // Window management belongs to the caller: the executor has no idea how many panes exist,
        // and giving it one would mean giving it the buffer list too.
        Action::SplitPane { .. }
        | Action::ClosePane
        | Action::CloseBuffer
        | Action::FocusPane { .. }
        | Action::NewFile
        | Action::RenameFile
        | Action::DeleteFile
        | Action::InstallLanguageServer
        | Action::ToggleTree => {
            return Outcome::Window(action.clone());
        }

        Action::MoveLine { down } => move_lines(state, *down),
        Action::DuplicateLine { down } => duplicate_lines(state, *down),
        Action::ToggleComment => toggle_comment(state),
        Action::DeleteWord { forward } => delete_word(state, *forward),
        Action::SelectNextOccurrence { all } => select_occurrence(state, *all),

        // The overlays belong to the caller: it owns the palette, the help panel and the menu, and
        // each of them takes the keyboard away from the buffer while it is open.
        Action::ToggleCommandPalette
        | Action::ToggleHelp
        | Action::ToggleMenu
        | Action::ToggleContextMenu => {
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
                state.set_mode(Mode::Normal);
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
                .mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));

            if operator == Operator::Change {
                state.set_mode(Mode::Insert);
            } else if leaving_visual {
                state.set_mode(Mode::Normal);
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
                .mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));
            if leaving_visual {
                state.set_mode(Mode::Normal);
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
        let _ =
            state.mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));
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
            state.mutate(|document| document.insert_newline());
        }
        InsertPosition::LineAbove => {
            state.document.move_to_line_edge(false, false);
            state.mutate(|document| document.insert("\n"));
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

    let _ = state.mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));
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

/// The lines each selection touches, merged and sorted.
///
/// Merged because two cursors on the same line are one line's worth of work, and a line moved twice
/// would move two rows. Every line-wise command below starts here.
fn selected_line_spans(state: &EditorState) -> Vec<(usize, usize)> {
    let text = state.document.text();
    let mut spans: Vec<(usize, usize)> = state
        .document
        .selections()
        .iter()
        .map(|selection| {
            let first = text.char_to_line(selection.start());
            // A selection ending exactly at a line start has not touched that line — it stops at the
            // break before it. Without this, selecting one whole line acts on two.
            let end = selection.end();
            let mut last = text.char_to_line(end);
            if last > first && end == text.line_to_char(last) {
                last -= 1;
            }
            (first, last)
        })
        .collect();

    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (first, last) in spans {
        match merged.last_mut() {
            Some(previous) if first <= previous.1 + 1 => previous.1 = previous.1.max(last),
            _ => merged.push((first, last)),
        }
    }
    merged
}

/// A line's text without its break.
fn line_text(text: &dacode_text::Text, line: usize) -> String {
    text.slice_to_string(text.line_range(line))
}

/// Rewrites whole lines `first..=last` and keeps the trailing break right.
///
/// The break is the whole difficulty: `line_range` includes it for every line but the last one in
/// the file, so a naive replacement either eats the final newline or grows one.
fn replace_lines(text: &dacode_text::Text, first: usize, last: usize, lines: &[String]) -> Edit {
    let region = line_range(text, first, last);
    let mut replacement = lines.join("\n");
    if last < text.last_line() {
        replacement.push('\n');
    }
    Edit::replace(region, replacement)
}

/// Moves the selected lines up or down by one — `alt-↑` / `alt-↓`.
///
/// The selection travels with the text. Anything else makes the second press act on lines you are no
/// longer looking at, which is the version of this feature that gets called broken.
fn move_lines(state: &mut EditorState, down: bool) {
    let text = state.document.text().clone();
    let spans = selected_line_spans(state);
    let mut edits = Vec::new();

    for (first, last) in &spans {
        let (first, last) = (*first, *last);
        // Already against the edge: that span does not move, and the others still do.
        if (down && last >= text.last_line()) || (!down && first == 0) {
            continue;
        }

        let (from, to) = if down {
            (first, last + 1)
        } else {
            (first - 1, last)
        };
        let mut contents: Vec<String> = (from..=to).map(|line| line_text(&text, line)).collect();
        if down {
            contents.rotate_right(1);
        } else {
            contents.rotate_left(1);
        }
        edits.push(replace_lines(&text, from, to, &contents));
    }

    if edits.is_empty() {
        return;
    }

    // The new selections are computed against the *old* text, in line and column, because a
    // whole-line swap leaves every column exactly where it was.
    let delta: isize = if down { 1 } else { -1 };
    let moved: Vec<(Point, Point)> = state
        .document
        .selections()
        .iter()
        .map(|selection| {
            let anchor = text.char_to_point(selection.anchor);
            let head = text.char_to_point(selection.head);
            (shift_line(anchor, delta), shift_line(head, delta))
        })
        .collect();

    let _ = state.mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));

    let text = state.document.text().clone();
    let selections: Vec<Selection> = moved
        .iter()
        .map(|(anchor, head)| {
            Selection::new(clamp_point(&text, *anchor), clamp_point(&text, *head))
        })
        .collect();
    if !selections.is_empty() {
        let primary = state.document.selections().primary_index();
        state
            .document
            .set_selections(SelectionSet::from_iter(selections, primary));
    }
}

fn shift_line(point: Point, delta: isize) -> Point {
    Point::new(point.line.saturating_add_signed(delta), point.column)
}

fn clamp_point(text: &dacode_text::Text, point: Point) -> CharOffset {
    let line = point.line.min(text.last_line());
    let column = point.column.min(text.line_len(line));
    text.point_to_char(Point::new(line, column))
}

/// Copies the selected lines above or below themselves — `shift-alt-↑` / `shift-alt-↓`.
fn duplicate_lines(state: &mut EditorState, down: bool) {
    let text = state.document.text().clone();
    let spans = selected_line_spans(state);
    if spans.is_empty() {
        return;
    }

    let mut edits = Vec::new();
    for (first, last) in &spans {
        let block: Vec<String> = (*first..=*last)
            .map(|line| line_text(&text, line))
            .collect();
        let mut copy = block.join("\n");
        copy.push('\n');
        edits.push(Edit::insert(text.line_to_char(*first), copy));
    }

    // Both copies are before the cursor's line after the insert, so the cursor ends up on the lower
    // one. That is what "duplicate down" means; for "up" the cursor belongs on the copy above, which
    // is the same row it started on.
    let points: Vec<(Point, Point)> = state
        .document
        .selections()
        .iter()
        .map(|selection| {
            (
                text.char_to_point(selection.anchor),
                text.char_to_point(selection.head),
            )
        })
        .collect();

    let _ = state.mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));

    if !down {
        let text = state.document.text().clone();
        let primary = state.document.selections().primary_index();
        let selections: Vec<Selection> = points
            .iter()
            .map(|(anchor, head)| {
                Selection::new(clamp_point(&text, *anchor), clamp_point(&text, *head))
            })
            .collect();
        state
            .document
            .set_selections(SelectionSet::from_iter(selections, primary));
    }
}

/// Comments or uncomments the selected lines — `ctrl-/`.
///
/// Uncommenting wins when *every* non-blank line is already commented, which is the rule that makes
/// the key a toggle over a mixed selection: a block where you commented half the lines by hand
/// becomes fully commented, and pressing again clears it.
fn toggle_comment(state: &mut EditorState) {
    let Some(language) = state
        .file_path
        .as_deref()
        .and_then(dacode_syntax::Language::from_path)
    else {
        // No language, no idea what a comment looks like here. Doing nothing beats guessing `//`
        // and writing it into a file where it means something else.
        return;
    };

    let text = state.document.text().clone();
    let spans = selected_line_spans(state);
    let lines: Vec<usize> = spans
        .iter()
        .flat_map(|(first, last)| *first..=*last)
        .collect();

    let content: Vec<(usize, String)> = lines
        .iter()
        .map(|line| (*line, line_text(&text, *line)))
        .collect();
    let interesting: Vec<&(usize, String)> = content
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    if interesting.is_empty() {
        return;
    }

    let comment = language.comment();
    let commented = interesting
        .iter()
        .all(|(_, text)| is_commented(text, comment));

    // Added at the shallowest indent of the block, not at each line's own, so the comment column
    // stays straight and the relative indentation inside the block survives.
    let column = interesting
        .iter()
        .map(|(_, text)| text.len() - text.trim_start().len())
        .min()
        .unwrap_or(0);

    let mut edits = Vec::new();
    for (line, source) in &content {
        if source.trim().is_empty() {
            continue;
        }
        let updated = if commented {
            uncomment(source, comment)
        } else {
            add_comment(source, comment, column)
        };
        if updated != *source {
            edits.push(Edit::replace(text.line_range(*line), updated));
        }
    }

    if !edits.is_empty() {
        let _ =
            state.mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));
    }
}

fn is_commented(source: &str, comment: dacode_syntax::Comment) -> bool {
    let trimmed = source.trim_start();
    match comment {
        dacode_syntax::Comment::Line(token) => trimmed.starts_with(token),
        dacode_syntax::Comment::Wrap(open, close) => {
            trimmed.starts_with(open) && source.trim_end().ends_with(close)
        }
    }
}

fn add_comment(source: &str, comment: dacode_syntax::Comment, column: usize) -> String {
    let (indent, rest) = source.split_at(column.min(source.len()));
    match comment {
        dacode_syntax::Comment::Line(token) => format!("{indent}{token} {rest}"),
        dacode_syntax::Comment::Wrap(open, close) => format!("{indent}{open} {rest} {close}"),
    }
}

fn uncomment(source: &str, comment: dacode_syntax::Comment) -> String {
    let indent: String = source.chars().take_while(|c| c.is_whitespace()).collect();
    let body = source.trim_start();
    match comment {
        dacode_syntax::Comment::Line(token) => {
            let rest = body.strip_prefix(token).unwrap_or(body);
            // The space this editor inserted after the token comes back off with it; one the user
            // typed does not, because their formatting is theirs.
            format!("{indent}{}", rest.strip_prefix(' ').unwrap_or(rest))
        }
        dacode_syntax::Comment::Wrap(open, close) => {
            let rest = body.strip_prefix(open).unwrap_or(body);
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            let rest = rest.trim_end();
            let rest = rest.strip_suffix(close).unwrap_or(rest);
            format!("{indent}{}", rest.strip_suffix(' ').unwrap_or(rest))
        }
    }
}

/// `ctrl-backspace` / `ctrl-delete`. The word motions already know where a word ends; this is the
/// same range an operator would take, deleted.
fn delete_word(state: &mut EditorState, forward: bool) {
    let motion = if forward {
        Motion::WordForward
    } else {
        Motion::WordBackward
    };
    let ranges = operator_ranges(state, Target::Motion { motion, count: 1 });
    if ranges.is_empty() {
        return;
    }
    let edits = ranges
        .iter()
        .map(|range| Edit::delete(range.clone()))
        .collect();
    let _ = state.mutate(|document| document.apply(Transaction::new(edits), EditKind::Discrete));
}

/// `ctrl-d`, and `ctrl-shift-l` for every occurrence at once.
///
/// Three behaviours in one key, which is what makes it feel like one gesture: from a caret it takes
/// the word, from a selection it adds the next copy of that selection, and it wraps at the end of
/// the file rather than stopping — the same reason `n` wraps.
fn select_occurrence(state: &mut EditorState, all: bool) {
    let text = state.document.text().clone();
    let selections = state.document.selections().clone();
    let primary = *selections.primary();

    // From a caret: take the word under it, and stop. The first press selects; the second adds.
    if primary.is_empty() {
        let object = dacode_input::TextObject::Word {
            around: false,
            big: false,
        };
        let mut updated: Vec<Selection> = Vec::new();
        for selection in selections.iter() {
            match dacode_input::resolve_object(&text, selection.head, object) {
                Some(range) if range.start < range.end => {
                    updated.push(Selection::new(range.start, range.end))
                }
                _ => updated.push(*selection),
            }
        }
        let index = selections.primary_index();
        state
            .document
            .set_selections(SelectionSet::from_iter(updated, index));
        return;
    }

    let query = text.slice_to_string(primary.start()..primary.end());
    let options = dacode_buffer::SearchOptions {
        case_sensitive: Some(true),
        ..Default::default()
    };
    let matches = dacode_buffer::find_all(&text, &query, &options);
    if matches.is_empty() {
        return;
    }

    if all {
        let found: Vec<Selection> = matches
            .iter()
            .map(|range| Selection::new(range.start, range.end))
            .collect();
        let last = found.len() - 1;
        state
            .document
            .set_selections(SelectionSet::from_iter(found, last));
        return;
    }

    let taken: Vec<(CharOffset, CharOffset)> = selections
        .iter()
        .map(|selection| (selection.start(), selection.end()))
        .collect();
    let next = matches
        .iter()
        .find(|range| range.start > primary.start() && !taken.contains(&(range.start, range.end)))
        .or_else(|| {
            matches
                .iter()
                .find(|range| !taken.contains(&(range.start, range.end)))
        });

    // Every occurrence is already selected: pressing again is a no-op rather than a surprise.
    let Some(next) = next else {
        return;
    };

    let mut updated: Vec<Selection> = selections.iter().copied().collect();
    updated.push(Selection::new(next.start, next.end));
    let last = updated.len() - 1;
    state
        .document
        .set_selections(SelectionSet::from_iter(updated, last));
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
    if let Some(found) = dacode_buffer::find_next(&state.search_matches, from, forward) {
        state.document.set_primary_caret(found.start);
    }
}

/// The word the cursor sits in or just after, for a bare project search.
fn word_under_cursor(state: &EditorState) -> String {
    let text = state.document.text();
    let object = dacode_input::TextObject::Word {
        around: false,
        big: false,
    };
    match dacode_input::resolve_object(text, state.cursor(), object) {
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
        .map(|diagnostic| dacode_lsp::range_from_lsp(&text, diagnostic.range).start)
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

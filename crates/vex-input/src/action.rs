//! The `Action` vocabulary — the single list of things the editor can do.
//!
//! Every user-visible operation is an `Action` value. Keymaps resolve to Actions, the command
//! palette lists them, macros record them, dot-repeat replays them, and tests drive them. That
//! one decision is what keeps Vim's composability from turning into a pile of special cases,
//! and it is why the enum is defined before any of the code that produces or consumes it.
//!
//! Because Lua config is in v1, this enum is also a **public scripting API**. Names are chosen
//! to be the ones a user will type, and they are expected to be stable.

use vex_theme::Mode;

/// A cursor movement. Motions are also the operands operators consume: `dw` is
/// `Delete` applied to `Motion::WordForward`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// Vertical motion is by *display* row, so it follows soft wrap.
    Up,
    Down,
    /// `w` — start of the next word.
    WordForward,
    /// `W` — next whitespace-delimited word.
    WordForwardBig,
    /// `b` — start of the previous word.
    WordBackward,
    WordBackwardBig,
    /// `e` — end of the current or next word.
    WordEnd,
    WordEndBig,
    LineStart,
    LineEnd,
    /// `^` — first non-whitespace character on the line.
    FirstNonBlank,
    FileStart,
    FileEnd,
    /// `G` with a count, or `:42`.
    ToLine(usize),
    /// `f`, `F`, `t`, `T`. `till` stops before the character instead of on it.
    FindChar {
        target: char,
        forward: bool,
        till: bool,
    },
    /// `%` — the bracket matching the one at or after the cursor.
    MatchingBracket,
    ParagraphForward,
    ParagraphBackward,
}

impl Motion {
    /// Whether a motion covers whole lines. `dj` deletes two lines, not a character range —
    /// getting this wrong is the classic way a Vim clone feels subtly wrong.
    pub fn is_linewise(self) -> bool {
        matches!(
            self,
            Motion::Up | Motion::Down | Motion::ToLine(_) | Motion::FileStart | Motion::FileEnd
        )
    }

    /// Whether the motion's end position is included in an operator's range. `dw` excludes the
    /// character the cursor lands on; `de` includes it.
    ///
    /// Both `f` and `t` are inclusive in Vim: `dfx` deletes through `x`, and `dtx` deletes
    /// through the character *before* `x` — the `till` difference is in where the motion lands,
    /// not in whether the landing character is taken. `%` is inclusive for the same reason:
    /// `d%` removes the whole balanced span, closing bracket included.
    pub fn is_inclusive(self) -> bool {
        matches!(
            self,
            Motion::WordEnd
                | Motion::WordEndBig
                | Motion::MatchingBracket
                | Motion::FindChar { forward: true, .. }
        )
    }
}

/// A structural region: `iw`, `a(`, `i"`. Tree-sitter adds `if`/`ic` (function, class) at M4;
/// the shape of this enum is what makes that an addition rather than a redesign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    /// `iw` / `aw`. `around` includes trailing whitespace.
    Word {
        around: bool,
        big: bool,
    },
    /// `i"` / `a"`, and the same for `'` and backtick.
    Quote {
        delimiter: char,
        around: bool,
    },
    /// `i(` / `a{` … `around` includes the delimiters themselves.
    Pair {
        open: char,
        close: char,
        around: bool,
    },
    Paragraph {
        around: bool,
    },
    /// The whole line, with or without its trailing newline.
    Line {
        around: bool,
    },
}

/// Something that transforms a range of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    /// Delete, then enter insert mode.
    Change,
    Yank,
    Indent,
    Outdent,
    Uppercase,
    Lowercase,
}

/// What an operator applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Motion {
        motion: Motion,
        count: usize,
    },
    Object(TextObject),
    /// `dd`, `yy` — the operator doubled.
    WholeLine {
        count: usize,
    },
    /// Visual mode: the operator applies to what is already selected.
    Selection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Move every selection's head, collapsing to a caret.
    Move {
        motion: Motion,
        count: usize,
    },
    /// Move the head while keeping the anchor — visual mode, and `v`-less selection commands.
    Extend {
        motion: Motion,
        count: usize,
    },

    Apply {
        operator: Operator,
        target: Target,
        register: Option<char>,
    },

    EnterMode(Mode),
    /// `a`, `A`, `o`, `O` — move, then enter insert mode, as one undoable intent.
    InsertAt(InsertPosition),

    InsertText(String),
    Newline,
    DeleteBackward,
    DeleteForward,
    /// `r<char>` — overwrite one character without leaving normal mode.
    ReplaceChar(char),

    Paste {
        register: Option<char>,
        before: bool,
        count: usize,
    },

    Undo,
    Redo,
    /// `.` — replay the last change.
    RepeatChange,

    /// Multi-cursor. Deliberately part of the same vocabulary as everything else, so it
    /// composes with operators instead of being a parallel code path.
    AddCaretAbove,
    AddCaretBelow,
    /// `s` in Helix terms: one caret per selected line.
    SplitSelectionIntoLines,
    KeepPrimarySelection,
    CollapseSelections,
    SelectLine {
        count: usize,
    },
    SelectAll,

    ToggleSoftWrap,
    ToggleTheme,
    IncreaseFontSize,
    DecreaseFontSize,

    Save,
    Quit,

    /// `/` and `?` — open the search prompt.
    StartSearch {
        forward: bool,
    },
    /// `n` / `N` — jump to the next or previous match of the last search.
    RepeatSearch {
        forward: bool,
    },
    /// `:` — open the command line.
    StartCommand,
    /// Project-wide search for the word under the cursor, or via `:grep`.
    SearchProject,

    /// `K` — ask the language server what is under the cursor.
    Hover,
    /// `gd` — jump to the definition of the symbol under the cursor.
    GotoDefinition,
    /// `]d` / `[d` — move to the next or previous diagnostic in this file.
    NextDiagnostic {
        forward: bool,
    },
    /// Dismiss a hover box or diagnostic message.
    DismissInfo,
    /// `ctrl-space` — ask the language server what could go here.
    RequestCompletion,
    /// `gr` — rename the symbol under the cursor, everywhere.
    Rename,
    /// Reformat the buffer through the language server.
    Format,

    /// `ctrl-w v` / `ctrl-w s` — show the current buffer in a second pane.
    SplitPane {
        vertical: bool,
    },
    /// `ctrl-w c` — close the focused pane. The last pane never closes.
    ClosePane,
    /// `ctrl-w w` and the directional keys — move focus between panes.
    FocusPane {
        forward: bool,
    },
    /// `ctrl-e` — show or hide the file tree.
    ToggleTree,
    /// `ga` in Neovim's spelling — what the server offers to do about the code under the cursor.
    CodeAction,
    /// `gu` — every use of the symbol under the cursor.
    FindReferences,
    /// Show or hide inlay hints.
    ToggleInlayHints,
    /// `gD` — list every diagnostic in the file, rather than walking them one at a time.
    ToggleDiagnosticsList,

    /// `za` — fold or unfold the block at the cursor.
    ToggleFold,
    /// `zR` / `zM` — open every fold, or close every foldable block.
    FoldAll {
        close: bool,
    },

    /// `q<register>` — start or stop recording.
    RecordMacro {
        register: char,
    },
    /// `@<register>`, or `@@` for the last one used.
    ReplayMacro {
        register: Option<char>,
    },
    /// `ctrl-o` / `ctrl-i` — walk the jumplist.
    Jump {
        forward: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    /// `i`
    BeforeCursor,
    /// `a`
    AfterCursor,
    /// `I`
    LineStart,
    /// `A`
    LineEnd,
    /// `o`
    LineBelow,
    /// `O`
    LineAbove,
}

impl Action {
    /// Resolves a configuration name to an action.
    ///
    /// This is the vocabulary config can reach: the actions that make sense as the whole meaning of
    /// a key. Operators and text objects are deliberately absent — `d` is not an action, it is half
    /// of one, and a binding that produced a dangling operator would leave the grammar mid-command.
    /// Motions are reachable through `move_*`, which collapses to a caret the way a plain motion
    /// does.
    ///
    /// Returns `None` for an unknown name rather than a default, so a typo in `init.lua` can be
    /// reported to the user instead of silently binding a key to nothing.
    pub fn from_name(name: &str) -> Option<Action> {
        use InsertPosition as At;
        use Motion as M;

        let motion = |motion: Motion| Action::Move { motion, count: 1 };

        Some(match name {
            // Motions.
            "move_left" => motion(M::Left),
            "move_right" => motion(M::Right),
            "move_up" => motion(M::Up),
            "move_down" => motion(M::Down),
            "move_word_forward" => motion(M::WordForward),
            "move_word_backward" => motion(M::WordBackward),
            "move_word_end" => motion(M::WordEnd),
            "move_line_start" => motion(M::LineStart),
            "move_line_end" => motion(M::LineEnd),
            "move_first_non_blank" => motion(M::FirstNonBlank),
            "move_file_start" => motion(M::FileStart),
            "move_file_end" => motion(M::FileEnd),
            "move_matching_bracket" => motion(M::MatchingBracket),
            "move_paragraph_forward" => motion(M::ParagraphForward),
            "move_paragraph_backward" => motion(M::ParagraphBackward),

            // Modes.
            "normal_mode" => Action::EnterMode(Mode::Normal),
            "insert_mode" => Action::EnterMode(Mode::Insert),
            "visual_mode" => Action::EnterMode(Mode::Visual),
            "replace_mode" => Action::EnterMode(Mode::Replace),
            "insert_before" => Action::InsertAt(At::BeforeCursor),
            "insert_after" => Action::InsertAt(At::AfterCursor),
            "insert_line_start" => Action::InsertAt(At::LineStart),
            "insert_line_end" => Action::InsertAt(At::LineEnd),
            "open_below" => Action::InsertAt(At::LineBelow),
            "open_above" => Action::InsertAt(At::LineAbove),

            // Editing.
            "newline" => Action::Newline,
            "delete_backward" => Action::DeleteBackward,
            "delete_forward" => Action::DeleteForward,
            "paste" => Action::Paste {
                register: None,
                before: false,
                count: 1,
            },
            "paste_before" => Action::Paste {
                register: None,
                before: true,
                count: 1,
            },
            "undo" => Action::Undo,
            "redo" => Action::Redo,
            "repeat_change" => Action::RepeatChange,

            // Selections.
            "add_caret_above" => Action::AddCaretAbove,
            "add_caret_below" => Action::AddCaretBelow,
            "split_selection_into_lines" => Action::SplitSelectionIntoLines,
            "keep_primary_selection" => Action::KeepPrimarySelection,
            "collapse_selections" => Action::CollapseSelections,
            "select_line" => Action::SelectLine { count: 1 },
            "select_all" => Action::SelectAll,

            // Appearance.
            "toggle_soft_wrap" => Action::ToggleSoftWrap,
            "toggle_theme" => Action::ToggleTheme,
            "increase_font_size" => Action::IncreaseFontSize,
            "decrease_font_size" => Action::DecreaseFontSize,

            // Files and search.
            "save" => Action::Save,
            "quit" => Action::Quit,
            "search" => Action::StartSearch { forward: true },
            "search_backward" => Action::StartSearch { forward: false },
            "search_next" => Action::RepeatSearch { forward: true },
            "search_previous" => Action::RepeatSearch { forward: false },
            "command" => Action::StartCommand,
            "search_project" => Action::SearchProject,

            // Language server.
            "hover" => Action::Hover,
            "goto_definition" => Action::GotoDefinition,
            "next_diagnostic" => Action::NextDiagnostic { forward: true },
            "previous_diagnostic" => Action::NextDiagnostic { forward: false },
            "dismiss_info" => Action::DismissInfo,
            "completion" => Action::RequestCompletion,
            "rename" => Action::Rename,
            "format" => Action::Format,
            "split_vertical" => Action::SplitPane { vertical: true },
            "split_horizontal" => Action::SplitPane { vertical: false },
            "close_pane" => Action::ClosePane,
            "next_pane" => Action::FocusPane { forward: true },
            "previous_pane" => Action::FocusPane { forward: false },
            "toggle_tree" => Action::ToggleTree,
            "code_action" => Action::CodeAction,
            "find_references" => Action::FindReferences,
            "toggle_inlay_hints" => Action::ToggleInlayHints,
            "toggle_diagnostics" => Action::ToggleDiagnosticsList,
            "toggle_fold" => Action::ToggleFold,
            "fold_all" => Action::FoldAll { close: true },
            "unfold_all" => Action::FoldAll { close: false },
            "jump_back" => Action::Jump { forward: false },
            "jump_forward" => Action::Jump { forward: true },
            "replay_macro" => Action::ReplayMacro { register: None },

            _ => return None,
        })
    }

    /// Whether replaying this action is what `.` should do.
    ///
    /// Only *changes* are repeatable: repeating a motion or a mode switch would make `.`
    /// unpredictable, which is worse than it doing nothing.
    pub fn is_change(&self) -> bool {
        match self {
            Action::Apply { operator, .. } => !matches!(operator, Operator::Yank),
            Action::InsertText(_)
            | Action::Newline
            | Action::DeleteBackward
            | Action::DeleteForward
            | Action::ReplaceChar(_)
            | Action::Paste { .. } => true,
            _ => false,
        }
    }
}

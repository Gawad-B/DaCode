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

//! The `Action` vocabulary — the single list of things the editor can do.
//!
//! Every user-visible operation is an `Action` value. Keymaps resolve to Actions, the command
//! palette lists them, macros record them, dot-repeat replays them, and tests drive them. That
//! one decision is what keeps Vim's composability from turning into a pile of special cases,
//! and it is why the enum is defined before any of the code that produces or consumes it.
//!
//! Because Lua config is in v1, this enum is also a **public scripting API**. Names are chosen
//! to be the ones a user will type, and they are expected to be stable.

use dacode_theme::Mode;

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

/// A structural region: `iw`, `a(`, `i"`, and from the syntax tree `af`, `at`, `aa`, `ac`.
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
    /// `af` / `it` / `aa` / `ic` — read from the syntax tree, not the text.
    ///
    /// This crate cannot see a tree, so [`crate::resolve_object`] answers `None` for these and the
    /// editor resolves them itself, from a tree whose version matches the buffer or not at all.
    ///
    /// `count` is the count typed with it, and means *the count-th enclosing one, outwards*: `2af`
    /// inside a closure is the function the closure is in. That includes arguments — `2aa` is the
    /// argument of the next list out that contains this one, which is how targets.vim and
    /// nvim-treesitter-textobjects both read a count on an object, rather than "this argument and
    /// the next one", which neither does and which would make `2af` and `2aa` count in different
    /// directions. Comments do not nest, so a count above one finds nothing there.
    Syntax {
        kind: SyntaxObject,
        around: bool,
        count: usize,
    },
}

/// The structural objects, by what they mean rather than by any grammar's node names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxObject {
    /// `f`
    Function,
    /// `t` — a class, struct, enum, trait, impl or interface.
    Type,
    /// `a` — one argument or parameter.
    Argument,
    /// `c`
    Comment,
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
    /// Select a text object outright: `vaf` in visual mode, and "select enclosing function" in
    /// the non-modal editor, which has no operator to put in front of one.
    SelectObject(TextObject),
    /// Grow every selection to the syntax node around it — the conventional editor's
    /// `shift-alt-→`.
    ExpandSelection,
    /// The inverse, `shift-alt-←`: back to what each selection was before the last expansion, or
    /// with no expansion to undo, down to the named node under the caret inside it.
    ShrinkSelection,

    ToggleSoftWrap,
    /// `zh` / `zl` — slide the view sideways along a long line, without moving the caret.
    ///
    /// Only means anything with soft wrap off; with it on there is nothing to the right of the
    /// viewport to look at.
    ScrollHorizontally {
        right: bool,
    },
    ToggleTheme,
    IncreaseFontSize,
    DecreaseFontSize,

    Save,
    /// `ctrl-shift-s` — write this buffer somewhere else, and keep editing it there.
    SaveAs,
    /// Write the buffer whenever it has been dirty for a moment, without being asked.
    ToggleAutosave,
    Quit,

    /// `/` and `?` — open the search prompt.
    StartSearch {
        forward: bool,
    },
    /// `n` / `N` — jump to the next or previous match of the last search.
    RepeatSearch {
        forward: bool,
    },
    /// `ctrl-h` — find and replace, in this file.
    ///
    /// Opens the prompt and stops there. The replacing itself is done by the prompt's own keys, as
    /// stepping between matches already is: both need two strings and a match the cursor is sitting
    /// on, neither of which a palette row or a macro replay can supply.
    StartReplace,
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

    /// `ctrl-k v` / `ctrl-k s` — show the current buffer in a second pane.
    SplitPane {
        vertical: bool,
    },
    /// `ctrl-k c` — close the focused pane. The last pane never closes.
    ClosePane,
    /// `ctrl-w` — close the file. Not the *pane* and not the editor: the buffer.
    ///
    /// `ctrl-w` in every mode of both grammars, because it is what every tabbed application closes a
    /// tab with; Vim's window commands moved to `ctrl-k` to make room. `ctrl-F4` is the other
    /// Windows convention, and the tab has a close button beside its name for everyone who would
    /// never have guessed either.
    CloseBuffer,
    /// `ctrl-k w` and the directional keys — move focus between panes.
    FocusPane {
        forward: bool,
    },
    /// `ctrl-e` — show or hide the file tree.
    ToggleTree,
    /// `ctrl-shift-e` — send the keys to the file tree, opening it first when it is closed.
    ///
    /// `ToggleTree` cannot be this: pressed with the tree already open it closes it, and the thing
    /// wanted is to *get back to it* after typing in the editor.
    FocusExplorer,
    /// `ctrl-1` — send the keys back to the text, leaving whatever is open where it is.
    FocusEditor,

    /// Create a file beside the one selected in the tree, or beside the open file.
    ///
    /// The tree was strictly read-only, which contradicts this project's own rule about features
    /// reachable only by a key — for the audience the whole mouse layer was built for.
    NewFile,
    /// Rename the selected file, on disk.
    RenameFile,
    /// Delete the selected file. Refuses once before it does.
    DeleteFile,
    /// Install the language server for this file, by running the command the editor already told
    /// you to type. Refuses once before it does: this runs a package manager.
    InstallLanguageServer,
    /// `ga` in Neovim's spelling — what the server offers to do about the code under the cursor.
    CodeAction,
    /// `gu` — every use of the symbol under the cursor.
    FindReferences,
    /// `]c` / `[c` — the next or previous region that differs from the last commit.
    GotoChange {
        forward: bool,
    },
    /// `]C` — put the change under the cursor back to how it was committed.
    RevertHunk,
    /// `gs` — every name in this file, as a searchable outline.
    GotoSymbol,
    /// `gS` / `ctrl-shift-t` — every name in the project, searched as you type.
    GotoSymbolInProject,
    /// Show or hide inlay hints.
    ToggleInlayHints,
    /// `gb` — a column in the gutter saying who last changed each visible line, and when.
    ToggleBlame,
    /// `gB` — the whole commit that last changed the caret's line: hash, author, date, message.
    ShowCommit,
    /// `gD` — list every diagnostic in the file, rather than walking them one at a time.
    ToggleDiagnosticsList,
    /// The source-control view in the sidebar.
    ShowSourceControl,
    /// Every message the editor has shown, newest first — so one that vanished at the next
    /// keystroke can still be read.
    ToggleNotifications,
    /// Copy a reference to the cursor's line — a link to it on the repository's host when there is
    /// one, `path:line` when there is not.
    Share,
    /// Run the project: its run command, typed into the terminal.
    Run,
    /// `ctrl-`` or `ctrl-t` — show the terminal and give it the keys, or, from the terminal, hide
    /// it and give them back. The two keys a focused terminal keeps from the shell for this.
    ToggleTerminal,
    /// Show the terminal and give it the keys, whatever has them now.
    FocusTerminal,
    /// Start another shell in a tab of its own beside the others, and show it.
    NewTerminal,
    /// The next terminal tab. `ctrl-pagedown` while a terminal has the keys.
    NextTerminal,
    /// The previous terminal tab. `ctrl-pageup` while a terminal has the keys.
    PreviousTerminal,
    /// End the shown terminal's shell and close its tab; closing the last hides the panel.
    KillTerminal,
    /// Copy the terminal's mouse selection. `ctrl-shift-c` while a terminal has the keys.
    TerminalCopy,
    /// Paste into the terminal, bracketed if the program asked. `ctrl-shift-v` there.
    TerminalPaste,

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
    /// `tab` in insert mode: expand the snippet before the cursor, or move to the next tab stop.
    /// Falls back to inserting an indent when neither applies.
    ExpandSnippet,
    /// `ctrl-k ctrl-o` — open a folder as the project.
    OpenFolder,
    /// `ctrl-,` — the settings tab: opens it, or shows it when it is already open.
    ToggleSettings,

    // ---- the conventional editor's own vocabulary ---------------------------------------
    //
    // Vim reaches these by composition — `ddp` moves a line, `yyp` duplicates one — so the modal
    // grammar never needed them as verbs. With the grammar off there is nothing to compose *with*,
    // and their absence is most of why the non-modal mode felt like the modal one with the good
    // parts removed. They are actions rather than special cases in the keymap so they compose the
    // other way too: bindable, in the palette, recorded by macros, repeatable with `.`.
    /// `alt-↑` / `alt-↓` — move the line or selected lines by one, carrying the selection with them.
    MoveLine {
        down: bool,
    },
    /// `shift-alt-↑` / `shift-alt-↓` — copy the line or selected lines above or below themselves.
    DuplicateLine {
        down: bool,
    },
    /// `ctrl-/` — comment the selected lines, or uncomment them if they are all already commented.
    ToggleComment,
    /// `ctrl-d` — select the word under the cursor; pressed again, add the next occurrence of what
    /// is selected as another cursor. `all` takes every occurrence at once (`ctrl-shift-l`).
    ///
    /// The one shortcut a VSCode user reaches for first, and the reason `ctrl-d` is not
    /// [`Action::AddCaretBelow`] in the non-modal keymap however convenient that was.
    SelectNextOccurrence {
        all: bool,
    },
    /// `ctrl-backspace` / `ctrl-delete` — delete to the word boundary.
    DeleteWord {
        forward: bool,
    },

    // ---- finding out what the editor can do ---------------------------------------------
    /// `ctrl-shift-p` — every command, searchable, each with the key that runs it.
    ToggleCommandPalette,
    /// `f1` — the keys that work right now, in a panel, without leaving the editor.
    ToggleHelp,
    /// `alt` or `f10` — the menu bar, which is also what the mouse clicks.
    ToggleMenu,
    /// `shift-f10`, or a right-click — the context menu at the cursor.
    ToggleContextMenu,
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
    /// Every name [`Action::from_name`] accepts.
    ///
    /// Written out rather than derived, because `from_name` is a match over string literals and
    /// nothing can enumerate that. Two tests hold the pair together: one that every name here
    /// resolves, and one in `commands` that every name here has a palette entry. Without the list
    /// there is no way to ask "what can this editor do?", which is exactly the question the command
    /// palette exists to answer.
    pub const NAMES: &'static [&'static str] = &[
        "move_left",
        "move_right",
        "move_up",
        "move_down",
        "move_word_forward",
        "move_word_backward",
        "move_word_end",
        "move_line_start",
        "move_line_end",
        "move_first_non_blank",
        "move_file_start",
        "move_file_end",
        "move_matching_bracket",
        "move_paragraph_forward",
        "move_paragraph_backward",
        "normal_mode",
        "insert_mode",
        "visual_mode",
        "replace_mode",
        "insert_before",
        "insert_after",
        "insert_line_start",
        "insert_line_end",
        "open_below",
        "open_above",
        "newline",
        "delete_backward",
        "delete_forward",
        "paste",
        "paste_before",
        "undo",
        "redo",
        "repeat_change",
        "add_caret_above",
        "add_caret_below",
        "split_selection_into_lines",
        "keep_primary_selection",
        "collapse_selections",
        "select_line",
        "select_all",
        "select_function",
        "select_type",
        "select_argument",
        "expand_selection",
        "shrink_selection",
        "toggle_soft_wrap",
        "scroll_left",
        "scroll_right",
        "toggle_theme",
        "increase_font_size",
        "decrease_font_size",
        "save",
        "save_as",
        "toggle_autosave",
        "quit",
        "search",
        "search_backward",
        "search_next",
        "replace",
        "search_previous",
        "command",
        "search_project",
        "hover",
        "goto_definition",
        "next_diagnostic",
        "previous_diagnostic",
        "dismiss_info",
        "completion",
        "rename",
        "format",
        "split_vertical",
        "split_horizontal",
        "close_pane",
        "close_buffer",
        "next_pane",
        "previous_pane",
        "toggle_tree",
        "focus_explorer",
        "focus_editor",
        "new_file",
        "rename_file",
        "delete_file",
        "install_language_server",
        "code_action",
        "find_references",
        "goto_symbol",
        "goto_symbol_in_project",
        "next_change",
        "previous_change",
        "revert_hunk",
        "toggle_inlay_hints",
        "toggle_blame",
        "show_commit",
        "toggle_diagnostics",
        "source_control",
        "notifications",
        "share",
        "run",
        "toggle_terminal",
        "focus_terminal",
        "new_terminal",
        "next_terminal",
        "previous_terminal",
        "kill_terminal",
        "terminal_copy",
        "terminal_paste",
        "toggle_fold",
        "fold_all",
        "unfold_all",
        "jump_back",
        "jump_forward",
        "replay_macro",
        "expand_snippet",
        "open_folder",
        "settings",
        "move_line_up",
        "move_line_down",
        "duplicate_line_up",
        "duplicate_line_down",
        "toggle_comment",
        "select_next_occurrence",
        "select_all_occurrences",
        "delete_word_backward",
        "delete_word_forward",
        "copy",
        "cut",
        "delete_line",
        "indent",
        "outdent",
        "command_palette",
        "help",
        "menu",
        "context_menu",
    ];

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
        let syntax = Action::select_syntax;

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
            "select_function" => syntax(SyntaxObject::Function),
            "select_type" => syntax(SyntaxObject::Type),
            "select_argument" => syntax(SyntaxObject::Argument),
            "expand_selection" => Action::ExpandSelection,
            "shrink_selection" => Action::ShrinkSelection,

            // Appearance.
            "toggle_soft_wrap" => Action::ToggleSoftWrap,
            "scroll_left" => Action::ScrollHorizontally { right: false },
            "scroll_right" => Action::ScrollHorizontally { right: true },
            "toggle_theme" => Action::ToggleTheme,
            "increase_font_size" => Action::IncreaseFontSize,
            "decrease_font_size" => Action::DecreaseFontSize,

            // Files and search.
            "save" => Action::Save,
            "save_as" => Action::SaveAs,
            "toggle_autosave" => Action::ToggleAutosave,
            "quit" => Action::Quit,
            "search" => Action::StartSearch { forward: true },
            "search_backward" => Action::StartSearch { forward: false },
            "search_next" => Action::RepeatSearch { forward: true },
            "search_previous" => Action::RepeatSearch { forward: false },
            "replace" => Action::StartReplace,
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
            "close_buffer" => Action::CloseBuffer,
            "next_pane" => Action::FocusPane { forward: true },
            "previous_pane" => Action::FocusPane { forward: false },
            "toggle_tree" => Action::ToggleTree,
            "focus_explorer" => Action::FocusExplorer,
            "focus_editor" => Action::FocusEditor,
            "new_file" => Action::NewFile,
            "rename_file" => Action::RenameFile,
            "delete_file" => Action::DeleteFile,
            "install_language_server" => Action::InstallLanguageServer,
            "code_action" => Action::CodeAction,
            "find_references" => Action::FindReferences,
            "next_change" => Action::GotoChange { forward: true },
            "previous_change" => Action::GotoChange { forward: false },
            "revert_hunk" => Action::RevertHunk,
            "goto_symbol" => Action::GotoSymbol,
            "goto_symbol_in_project" => Action::GotoSymbolInProject,
            "toggle_inlay_hints" => Action::ToggleInlayHints,
            "toggle_blame" => Action::ToggleBlame,
            "show_commit" => Action::ShowCommit,
            "toggle_diagnostics" => Action::ToggleDiagnosticsList,
            "source_control" => Action::ShowSourceControl,
            "notifications" => Action::ToggleNotifications,
            "share" => Action::Share,
            "run" => Action::Run,
            "toggle_terminal" => Action::ToggleTerminal,
            "focus_terminal" => Action::FocusTerminal,
            "new_terminal" => Action::NewTerminal,
            "next_terminal" => Action::NextTerminal,
            "previous_terminal" => Action::PreviousTerminal,
            "kill_terminal" => Action::KillTerminal,
            "terminal_copy" => Action::TerminalCopy,
            "terminal_paste" => Action::TerminalPaste,
            "toggle_fold" => Action::ToggleFold,
            "fold_all" => Action::FoldAll { close: true },
            "unfold_all" => Action::FoldAll { close: false },
            "jump_back" => Action::Jump { forward: false },
            "jump_forward" => Action::Jump { forward: true },
            "replay_macro" => Action::ReplayMacro { register: None },
            "expand_snippet" => Action::ExpandSnippet,
            "open_folder" => Action::OpenFolder,
            "settings" => Action::ToggleSettings,

            // The conventional vocabulary.
            "move_line_up" => Action::MoveLine { down: false },
            "move_line_down" => Action::MoveLine { down: true },
            "duplicate_line_up" => Action::DuplicateLine { down: false },
            "duplicate_line_down" => Action::DuplicateLine { down: true },
            "toggle_comment" => Action::ToggleComment,
            "select_next_occurrence" => Action::SelectNextOccurrence { all: false },
            "select_all_occurrences" => Action::SelectNextOccurrence { all: true },
            "delete_word_backward" => Action::DeleteWord { forward: false },
            "delete_word_forward" => Action::DeleteWord { forward: true },
            // The clipboard verbs by the names everyone knows them by. `y` and `d` are Vim's words
            // for these, and a menu row saying "yank" to someone who chose the conventional editor
            // is the editor showing off rather than helping.
            "copy" => Action::Apply {
                operator: Operator::Yank,
                target: Target::Selection,
                register: None,
            },
            "cut" => Action::Apply {
                operator: Operator::Delete,
                target: Target::Selection,
                register: None,
            },
            "delete_line" => Action::Apply {
                operator: Operator::Delete,
                target: Target::WholeLine { count: 1 },
                register: None,
            },
            "indent" => Action::Apply {
                operator: Operator::Indent,
                target: Target::Selection,
                register: None,
            },
            "outdent" => Action::Apply {
                operator: Operator::Outdent,
                target: Target::Selection,
                register: None,
            },

            // Finding out what the editor can do.
            "command_palette" => Action::ToggleCommandPalette,
            "help" => Action::ToggleHelp,
            "menu" => Action::ToggleMenu,
            "context_menu" => Action::ToggleContextMenu,

            _ => return None,
        })
    }

    /// What "select the enclosing function" means from a menu or a non-modal key.
    ///
    /// The whole function or type, as `vaf` takes it — the inside alone would leave out the
    /// signature most people mean by "the function". An argument is the other way round: "this
    /// argument" is the argument, and the comma `aa` takes with it is a detail of deleting one.
    pub fn select_syntax(kind: SyntaxObject) -> Action {
        let around = kind != SyntaxObject::Argument;
        Action::SelectObject(TextObject::Syntax {
            kind,
            around,
            count: 1,
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
            | Action::Paste { .. }
            | Action::MoveLine { .. }
            | Action::DuplicateLine { .. }
            | Action::ToggleComment
            | Action::DeleteWord { .. } => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_advertised_name_resolves() {
        // `NAMES` is public API — config, the palette and the help panel all read it — so a name in
        // it that `from_name` does not accept is a command the editor advertises and cannot run.
        for name in Action::NAMES {
            assert!(
                Action::from_name(name).is_some(),
                "`{name}` is advertised but does not resolve"
            );
        }
    }

    #[test]
    fn an_unknown_name_is_refused_rather_than_defaulted() {
        // A typo in init.lua has to be reportable. Silently binding the key to something would be
        // the worst of both: no error, and a key that does the wrong thing.
        assert!(Action::from_name("definitely_not_a_command").is_none());
    }

    #[test]
    fn the_name_list_has_no_duplicates() {
        let mut names = Action::NAMES.to_vec();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(total, names.len(), "a name is listed twice");
    }
}

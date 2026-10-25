//! Undo history.
//!
//! A stack of inverse transactions, which is all undo can be once every mutation goes through
//! [`dacode_text::Transaction`]. The only real design decision here is *grouping*: undo must move
//! in units that match the user's intent, not in units that match the implementation. Typing a
//! word is one undo; typing a word, moving, and typing another is two.

use dacode_text::{SelectionSet, Transaction};

/// One undoable step: the transaction that reverses an edit, plus the selections to restore.
#[derive(Debug, Clone)]
pub struct Step {
    pub inverse: Transaction,
    /// Selections as they were *before* the edit this reverses — restoring them is what makes
    /// undo feel like going back rather than like a separate edit.
    pub selections_before: SelectionSet,
}

/// What kind of edit produced a step, used to decide whether it can join the previous one.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EditKind {
    /// Typing. Consecutive insertions coalesce.
    Insert,
    /// Backspace/delete. Consecutive deletions coalesce, but never with insertions.
    Delete,
    /// Anything structural (paste, replace, formatting). Always its own group.
    Discrete,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Vec<Step>>,
    redo: Vec<Vec<Step>>,
    /// The kind of the last edit, for coalescing. `None` after an explicit boundary.
    last_kind: Option<EditKind>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a step, joining the current group when the edit kind allows it.
    pub fn record(&mut self, step: Step, kind: EditKind) {
        // Any new edit invalidates the redo stack — the classic linear-history model. A tree
        // (like Vim's undo tree) is a deliberate non-goal for v1.
        self.redo.clear();

        let can_coalesce =
            kind != EditKind::Discrete && self.last_kind == Some(kind) && !self.undo.is_empty();

        if can_coalesce {
            // Inverses are applied in reverse, so a later edit's inverse must run first.
            self.undo.last_mut().unwrap().insert(0, step);
        } else {
            self.undo.push(vec![step]);
        }

        self.last_kind = Some(kind);
    }

    /// Ends the current group, so the next edit starts a new undo step. Called on mode changes
    /// and cursor movement — the points where the user's intent visibly changes.
    pub fn end_group(&mut self) {
        self.last_kind = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Takes the next group to undo. The caller applies it and hands back the resulting
    /// inverses via [`Self::push_redo`].
    pub fn pop_undo(&mut self) -> Option<Vec<Step>> {
        self.last_kind = None;
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<Vec<Step>> {
        self.last_kind = None;
        self.redo.pop()
    }

    pub fn push_redo(&mut self, group: Vec<Step>) {
        self.redo.push(group);
    }

    pub fn push_undo(&mut self, group: Vec<Step>) {
        self.undo.push(group);
    }

    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

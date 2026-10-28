//! `dacode-buffer` — a document: text, selections, version, undo history.
//!
//! This is the layer that owns *mutation*. Every editing operation is expressed as a
//! [`Transaction`] over the whole selection set, so multi-cursor editing is the default path
//! rather than a special case, and undo, dot-repeat and stale-result invalidation all come out
//! of the same primitive.
//!
//! The version counter is the other half of the 60fps contract: it increments on every edit,
//! background services tag their results with the version they saw, and anything behind the
//! current version is discarded rather than applied.

pub mod history;
pub mod pairs;
pub mod search;

pub use history::{EditKind, History, Step};
pub use pairs::{NewlineAction, PairConfig, TypeAction};
pub use search::{SearchOptions, find_all, find_next, index_of};

use dacode_text::{CharOffset, Edit, Point, Selection, SelectionSet, Text, Transaction, Version};
use std::collections::VecDeque;

/// One applied transaction, as background services need to see it.
///
/// The `text` snapshot is what makes the sequence usable: an incremental reparse needs the text
/// as it was *after* each change, and a consumer that only ever sees the latest text cannot
/// reconstruct the intermediate states its edits were expressed against.
#[derive(Debug, Clone)]
pub struct Change {
    pub version: Version,
    pub edits: Vec<Edit>,
    pub text: Text,
}

/// How many undrained changes to keep. Past this the queue is dropped and consumers are told to
/// resynchronize — bounded memory beats a perfect history nobody is reading.
const MAX_PENDING_CHANGES: usize = 512;

pub struct Document {
    text: Text,
    selections: SelectionSet,
    version: Version,
    history: History,
    /// Set when the buffer differs from what is on disk.
    dirty: bool,
    /// Applied changes not yet collected by background services.
    pending: VecDeque<Change>,
    /// Set when the pending queue overflowed, so consumers reparse from scratch instead of
    /// applying an incomplete edit sequence to a stale tree.
    needs_resync: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self::from_text(Text::new())
    }
}

impl Document {
    pub fn from_text(text: Text) -> Self {
        Self {
            text,
            selections: SelectionSet::default(),
            version: Version(1),
            history: History::new(),
            dirty: false,
            pending: VecDeque::new(),
            needs_resync: false,
        }
    }

    // Mirrors `Text::from_str`; `FromStr` is the wrong trait because loading is infallible.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(contents: &str) -> Self {
        Self::from_text(Text::from_str(contents))
    }

    /// Continues version numbering from `after`, for a buffer that replaced another.
    ///
    /// Document versions are what every asynchronous consumer uses to tell fresh results from stale
    /// ones, and the language-server protocol requires them to *increase*. Reloading a file changed
    /// on disk builds a new `Document`, which would otherwise restart at 1 — telling a server that
    /// had already seen version 5 to go backwards, and making every diagnostic it returns look stale
    /// forever after.
    pub fn resume_from(&mut self, after: Version) {
        if after >= self.version {
            self.version = after.next();
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    /// A cheap structural clone for background work. Handing services a snapshot plus its
    /// version is what lets them run without locking the document the user is typing into.
    pub fn snapshot(&self) -> (Text, Version) {
        (self.text.clone(), self.version)
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn selections(&self) -> &SelectionSet {
        &self.selections
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Marks the buffer as differing from disk without recording an edit.
    ///
    /// For recovered content: the text genuinely differs from the file, but it arrived whole rather
    /// than through a transaction, so there is nothing to undo back to.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    fn record_change(&mut self, edits: Vec<Edit>) {
        if self.pending.len() >= MAX_PENDING_CHANGES {
            self.pending.clear();
            self.needs_resync = true;
        }
        self.pending.push_back(Change {
            version: self.version,
            edits,
            text: self.text.clone(),
        });
    }

    /// Collects the changes applied since the last call, oldest first.
    ///
    /// Consumers replay these in order; each one's edits are expressed against the previous
    /// one's text.
    pub fn take_changes(&mut self) -> Vec<Change> {
        self.pending.drain(..).collect()
    }

    /// The changes applied since `version`, borrowed rather than taken.
    ///
    /// The queue has one consumer that drains it — the syntax worker, which replays the changes in
    /// order and cannot have any of them taken from underneath it. Anything else that needs the
    /// same edits in the same coordinates has to look without touching, which is what this is for:
    /// the display map's folds are line numbers, and only these edits say how far the lines moved.
    ///
    /// Empty when nothing changed. Also empty if the queue was truncated past `version`, which is
    /// indistinguishable from here — callers that must know should compare
    /// [`Document::version`] themselves and treat the gap as "start over", the same as
    /// [`Document::take_needs_resync`].
    pub fn changes_since(&self, version: Version) -> impl ExactSizeIterator<Item = &Change> {
        let start = self
            .pending
            .partition_point(|change| change.version <= version);
        self.pending.range(start..)
    }

    /// Whether the change sequence was truncated, meaning a consumer must start over.
    pub fn take_needs_resync(&mut self) -> bool {
        std::mem::take(&mut self.needs_resync)
    }

    /// Ends the current undo group. Call on mode changes and deliberate cursor moves.
    pub fn end_undo_group(&mut self) {
        self.history.end_group();
    }

    pub fn set_selections(&mut self, selections: SelectionSet) {
        self.selections = selections.clamp(&self.text);
        // Moving the cursor ends the typing run: "type, move, type" must be two undos.
        self.history.end_group();
    }

    pub fn set_primary_caret(&mut self, at: CharOffset) {
        self.set_selections(SelectionSet::single(Selection::caret(self.text.clamp(at))));
    }

    // ---- editing -----------------------------------------------------------------------

    /// Applies a transaction, recording its inverse. The single funnel for all mutation.
    pub fn apply(
        &mut self,
        transaction: Transaction,
        kind: EditKind,
    ) -> Result<(), dacode_text::EditError> {
        if transaction.is_empty() {
            return Ok(());
        }

        let selections_before = self.selections.clone();
        let inverse = transaction.apply(&mut self.text)?;

        // Selections move with the text they sit in.
        self.selections = self
            .selections
            .transform(&transaction.edits)
            .clamp(&self.text);

        self.version = self.version.next();
        self.dirty = true;
        self.record_change(transaction.edits);
        self.history.record(
            Step {
                inverse,
                selections_before,
            },
            kind,
        );

        Ok(())
    }

    /// Inserts text at every selection, replacing any selected range.
    pub fn insert(&mut self, text: &str) {
        let edits = self.selections.replace_edits(text);
        let transaction = Transaction::new(edits);
        // Infallible: `replace_edits` derives its ranges from the selection-set invariant
        // (sorted, disjoint, in bounds), which is exactly what `validate` checks.
        let _ = self.apply(transaction, EditKind::Insert);

        // Each caret ends up after its own inserted text — `Bias::Right` semantics, which
        // `Selection::transform` already applies to the start of a collapsed range.
        let inserted = text.chars().count();
        if inserted > 0 {
            self.selections = self.selections.map(|selection| {
                if selection.is_caret() {
                    Selection::caret(CharOffset(selection.head.get()))
                } else {
                    selection.collapse_to_head()
                }
            });
        }
    }

    /// Backspace: deletes the grapheme before each caret, or the selected range.
    pub fn delete_backward(&mut self) {
        let text = self.text.clone();
        let edits = self.edits_for_deletion(|selection| {
            if selection.is_caret() {
                let from = text.prev_grapheme(selection.head);
                from..selection.head
            } else {
                selection.range()
            }
        });
        let _ = self.apply(Transaction::new(edits), EditKind::Delete);
    }

    /// Delete key: deletes the grapheme after each caret, or the selected range.
    pub fn delete_forward(&mut self) {
        let text = self.text.clone();
        let edits = self.edits_for_deletion(|selection| {
            if selection.is_caret() {
                let to = text.next_grapheme(selection.head);
                selection.head..to
            } else {
                selection.range()
            }
        });
        let _ = self.apply(Transaction::new(edits), EditKind::Delete);
    }

    /// Builds deletion edits, keeping them sorted, disjoint and non-empty.
    ///
    /// Adjacent carets are the reason this needs care: two carets one character apart both want
    /// to delete backward into the same character, which would produce overlapping edits and a
    /// rejected transaction.
    fn edits_for_deletion(
        &self,
        mut range_of: impl FnMut(&Selection) -> std::ops::Range<CharOffset>,
    ) -> Vec<Edit> {
        let mut edits: Vec<Edit> = Vec::with_capacity(self.selections.len());
        let mut previous_end = CharOffset::ZERO;

        for selection in self.selections.iter() {
            let range = range_of(selection);
            let start = range.start.max(previous_end);
            if start >= range.end {
                continue; // fully swallowed by the previous deletion, or a no-op at the edge
            }
            previous_end = range.end;
            edits.push(Edit::delete(start..range.end));
        }

        edits
    }

    /// Types one character, pairing brackets and quotes where it makes sense.
    ///
    /// Multi-cursor safe: each caret is decided independently and the results become one
    /// transaction, so `(` with three cursors produces three pairs and one undo.
    pub fn type_char(&mut self, typed: char, config: &PairConfig) -> TypeAction {
        self.type_char_indented(typed, config, None)
    }

    /// [`Document::type_char`], with the electric-dedent indentation already decided by someone who
    /// knows more than this crate can — the editor's syntax tree, when it is fresh.
    ///
    /// `dedent` replaces only the *search for the opener*. The conditions under which a closer is
    /// reindented at all — typed alone on a blank line, as a literal — are still decided here, so a
    /// caller cannot make a `}` typed after code move that code.
    pub fn type_char_indented(
        &mut self,
        typed: char,
        config: &PairConfig,
        dedent: Option<String>,
    ) -> TypeAction {
        let text = self.text.clone();
        let primary = self.selections.primary().head;

        // The primary caret decides, and every caret follows it. Deciding per caret would be more
        // clever and less predictable: the same keystroke would insert different things in different
        // places, and the user only sees one of them.
        let action = if self.selections.primary().is_caret() {
            pairs::on_type(&text, primary, typed, config)
        } else {
            // With a selection, the pair wraps it — that is the useful behavior, and stepping over
            // makes no sense when there is something selected.
            match config.closer_for(typed) {
                Some(close) => TypeAction::Pair { open: typed, close },
                None if config.is_quote(typed) => TypeAction::Pair {
                    open: typed,
                    close: typed,
                },
                None => TypeAction::Literal,
            }
        };

        match action {
            TypeAction::Literal => {
                // Electric dedent: a closer alone on its line pulls the line back to its opener.
                let indent = match dedent {
                    Some(indent) => pairs::dedent_to(&text, primary, typed, config, indent),
                    None => pairs::dedent_for_closer(&text, primary, typed, config),
                };
                if let Some(indent) = indent {
                    self.reindent_line(&indent);
                }
                self.insert(&typed.to_string());
            }
            TypeAction::StepOver => {
                self.selections = self
                    .selections
                    .map(|selection| Selection::caret(text.next_grapheme(selection.head)));
                self.history.end_group();
            }
            TypeAction::Pair { open, close } => {
                if self.selections.primary().is_caret() {
                    // No electric dedent here. This arm is reached with an *opening* delimiter, and
                    // `dedent_for_closer` returns on its first line for anything that is not a
                    // closer — so the call that stood here could never do anything, while implying
                    // a second live path that had to be kept in step with the first.
                    self.insert(&format!("{open}{close}"));
                    // Land between the halves.
                    self.selections = self
                        .selections
                        .map(|selection| Selection::caret(self.text.prev_grapheme(selection.head)));
                } else {
                    // Wrap each selection: one transaction, two edits per selection.
                    let mut edits = Vec::with_capacity(self.selections.len() * 2);
                    for selection in self.selections.iter() {
                        edits.push(Edit::insert(selection.start(), open.to_string()));
                        edits.push(Edit::insert(selection.end(), close.to_string()));
                    }
                    let _ = self.apply(Transaction::new(edits), EditKind::Discrete);
                }
            }
        }

        action
    }

    /// Replaces the current line's leading whitespace.
    fn reindent_line(&mut self, indent: &str) {
        let point = self.text.char_to_point(self.selections.primary().head);
        let start = self.text.line_to_char(point.line);
        let content = self.text.slice_to_string(self.text.line_range(point.line));
        let existing = content
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();

        let edit = Edit::replace(
            start..CharOffset(start.get() + existing),
            indent.to_string(),
        );
        let _ = self.apply(Transaction::new(vec![edit]), EditKind::Discrete);
    }

    /// Backspace that removes both halves of an empty pair.
    ///
    /// **Decided per caret**, unlike [`Document::type_char`]. That function's rule — the primary
    /// decides and every caret follows — is defensible because every caret then inserts the *same*
    /// text. It does not carry over to deleting: this asked whether the *primary* sat between `(`
    /// and `)` and then took two graphemes at every caret, so with the primary inside a pair, a
    /// second caret in the middle of a word ate the character after it as well as the one before.
    /// A caret at offset zero deleted forwards outright.
    ///
    /// Going through [`Document::edits_for_deletion`] rather than mapping the edits by hand is the
    /// other half. That helper's whole purpose is adjacent carets, which here produced `n-1..n+1`
    /// and `n..n+2` — overlapping, so the transaction was refused and the result discarded by a
    /// `let _ =`. Backspace silently did nothing for the entire selection set.
    pub fn delete_backward_pairs(&mut self, config: &PairConfig) {
        let text = self.text.clone();
        let edits = self.edits_for_deletion(|selection| {
            if !selection.is_caret() {
                // A range selection is deleted, always. It used to be filtered out on this branch
                // and deleted on the other, so what backspace did to selected text depended on
                // whether an unrelated caret happened to be sitting on a pair.
                return selection.range();
            }
            if pairs::deletes_pair(&text, selection.head, config) {
                return text.prev_grapheme(selection.head)..text.next_grapheme(selection.head);
            }
            text.prev_grapheme(selection.head)..selection.head
        });
        let _ = self.apply(Transaction::new(edits), EditKind::Delete);
    }

    /// A newline that indents, and that opens a block when the cursor sits inside a pair.
    pub fn insert_newline_smart(&mut self, config: &PairConfig) {
        self.insert_newline_indented(config, None);
    }

    /// [`Document::insert_newline_smart`], with the new line's indentation decided by the caller —
    /// the editor's syntax tree, when it is fresh and can answer.
    ///
    /// Between a pair (`{|}`) the lexical rule keeps the decision: it puts the closer on a line of
    /// its own as well, which is two lines' indentation from one rule, and the tree would only ever
    /// agree with it there.
    pub fn insert_newline_indented(&mut self, config: &PairConfig, indent: Option<String>) {
        let text = self.text.clone();
        let primary = self.selections.primary().head;
        let mut action = pairs::on_newline(&text, primary, config);
        if action.closing_line.is_none()
            && let Some(indent) = indent
        {
            action.indent = indent;
        }

        match &action.closing_line {
            // Between `{` and `}`: two newlines, and the cursor lands on the blank one.
            Some(closing) => {
                let insertion = format!("\n{}\n{}", action.indent, closing);
                let edits = self.selections.replace_edits(&insertion);
                let _ = self.apply(Transaction::new(edits), EditKind::Insert);

                let back = 1 + closing.chars().count();
                self.selections = self.selections.map(|selection| {
                    let mut head = selection.head;
                    for _ in 0..back {
                        head = self.text.prev_grapheme(head);
                    }
                    Selection::caret(head)
                });
            }
            None => {
                let insertion = format!("\n{}", action.indent);
                let edits = self.selections.replace_edits(&insertion);
                let _ = self.apply(Transaction::new(edits), EditKind::Insert);
                self.selections = self
                    .selections
                    .map(|selection| Selection::caret(selection.head));
            }
        }
    }

    /// Inserts a newline at every caret, carrying the current line's leading whitespace.
    ///
    /// Real indentation comes from tree-sitter at M4; matching the previous line is the
    /// behavior that makes the editor usable before then.
    pub fn insert_newline(&mut self) {
        let mut inserts: Vec<Edit> = Vec::with_capacity(self.selections.len());

        for selection in self.selections.iter() {
            let point = self.text.char_to_point(selection.start());
            let line_start = self.text.line_to_char(point.line);
            let indent: String = self
                .text
                .slice_to_string(line_start..CharOffset(line_start.get() + point.column))
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();

            inserts.push(Edit::replace(selection.range(), format!("\n{indent}")));
        }

        let _ = self.apply(Transaction::new(inserts), EditKind::Insert);
        self.selections = self.selections.map(|selection| {
            if selection.is_caret() {
                selection
            } else {
                selection.collapse_to_head()
            }
        });
    }

    // ---- undo / redo -------------------------------------------------------------------

    pub fn undo(&mut self) -> bool {
        let Some(group) = self.history.pop_undo() else {
            return false;
        };

        let mut redo_group = Vec::with_capacity(group.len());
        let mut restore = None;

        for step in group {
            let applied = step.inverse.edits.clone();
            match step.inverse.apply(&mut self.text) {
                Ok(inverse) => {
                    // The first step's "before" selections are the ones to land on.
                    restore = Some(step.selections_before.clone());
                    redo_group.push(Step {
                        inverse,
                        selections_before: step.selections_before,
                    });
                    self.version = self.version.next();
                    self.record_change(applied);
                }
                Err(_) => return false,
            }
        }

        if let Some(selections) = restore {
            self.selections = selections.clamp(&self.text);
        }
        self.dirty = true;
        // Redo groups replay in the opposite order.
        redo_group.reverse();
        self.history.push_redo(redo_group);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(group) = self.history.pop_redo() else {
            return false;
        };

        let mut undo_group = Vec::with_capacity(group.len());
        for step in group {
            let applied = step.inverse.edits.clone();
            match step.inverse.apply(&mut self.text) {
                Ok(inverse) => {
                    undo_group.push(Step {
                        inverse,
                        selections_before: step.selections_before,
                    });
                    self.version = self.version.next();
                    self.record_change(applied);
                }
                Err(_) => return false,
            }
        }

        self.selections = self.selections.clamp(&self.text);
        self.dirty = true;
        undo_group.reverse();
        self.history.push_undo(undo_group);
        true
    }

    // ---- motion ------------------------------------------------------------------------
    //
    // Motions live here rather than in the input layer because they are document queries, not
    // key handling. The input layer resolves keys into these.

    pub fn move_grapheme(&mut self, forward: bool, extend: bool) {
        let text = self.text.clone();
        self.selections = self.selections.map(|selection| {
            let head = if forward {
                text.next_grapheme(selection.head)
            } else {
                text.prev_grapheme(selection.head)
            };
            if extend {
                selection.extend_to(head)
            } else {
                Selection::caret(head)
            }
        });
        self.history.end_group();
    }

    /// Vertical motion by *buffer* line. Display-aware vertical motion (which moves by wrapped
    /// row) belongs to the display layer, which owns the wrap geometry.
    pub fn move_line(&mut self, delta: isize, extend: bool) {
        let text = self.text.clone();
        self.selections = self.selections.map(|selection| {
            let point = text.char_to_point(selection.head);
            let line = point
                .line
                .saturating_add_signed(delta)
                .min(text.last_line());
            let head = text.point_to_char(Point::new(line, point.column));
            if extend {
                selection.extend_to(head)
            } else {
                Selection::caret(head)
            }
        });
        self.history.end_group();
    }

    pub fn move_to_line_edge(&mut self, end: bool, extend: bool) {
        let text = self.text.clone();
        self.selections = self.selections.map(|selection| {
            let point = text.char_to_point(selection.head);
            let column = if end { text.line_len(point.line) } else { 0 };
            let head = text.point_to_char(Point::new(point.line, column));
            if extend {
                selection.extend_to(head)
            } else {
                Selection::caret(head)
            }
        });
        self.history.end_group();
    }

    /// Adds a caret on the line above or below the primary — the multi-cursor primitive.
    pub fn add_caret_vertically(&mut self, delta: isize) {
        let primary = *self.selections.primary();
        let point = self.text.char_to_point(primary.head);
        let line = point.line.saturating_add_signed(delta);
        if line > self.text.last_line() {
            return;
        }

        let head = self.text.point_to_char(Point::new(line, point.column));
        let mut all: Vec<Selection> = self.selections.iter().copied().collect();
        all.push(Selection::caret(head));
        let primary_index = all.len() - 1;
        self.selections = SelectionSet::from_iter(all, primary_index);
        self.history.end_group();
    }

    pub fn collapse_selections(&mut self) {
        self.selections = self
            .selections
            .map(|selection| selection.collapse_to_head());
        // Keep only the primary: Esc should return to a single cursor.
        let primary = *self.selections.primary();
        self.selections = SelectionSet::single(primary);
        self.history.end_group();
    }
}

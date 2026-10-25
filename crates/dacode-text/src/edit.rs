//! Edits and transactions — the **only** path by which text may change.
//!
//! Every mutation is a [`Transaction`]: an ordered set of disjoint edits plus the selections
//! before and after. Applying one returns its exact inverse, so undo is a stack of inverses,
//! multi-cursor is a transaction with many edits, and dot-repeat is a replayable value.
//! If anything mutated the rope outside this path, undo and stale-result invalidation would
//! both silently break.

use crate::coords::CharOffset;
use crate::selection::SelectionSet;
use crate::text::Text;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("edit range {start:?}..{end:?} is inverted")]
    InvertedRange { start: CharOffset, end: CharOffset },
    #[error("edit range {start:?}..{end:?} is out of bounds (len {len:?})")]
    OutOfBounds {
        start: CharOffset,
        end: CharOffset,
        len: CharOffset,
    },
    #[error("edits are not sorted or overlap at {at:?}")]
    Overlapping { at: CharOffset },
}

/// A single replacement of a char range with new text. Insertion is an empty range; deletion
/// is empty text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<CharOffset>,
    pub text: String,
}

impl Edit {
    pub fn replace(range: Range<CharOffset>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }

    pub fn insert(at: CharOffset, text: impl Into<String>) -> Self {
        Self {
            range: at..at,
            text: text.into(),
        }
    }

    pub fn delete(range: Range<CharOffset>) -> Self {
        Self {
            range,
            text: String::new(),
        }
    }

    /// Chars removed by this edit.
    pub fn old_len(&self) -> usize {
        self.range.end.get().saturating_sub(self.range.start.get())
    }

    /// Chars inserted by this edit.
    pub fn new_len(&self) -> usize {
        self.text.chars().count()
    }

    /// Net change in document length.
    pub fn delta(&self) -> isize {
        self.new_len() as isize - self.old_len() as isize
    }
}

/// An atomic change to a document.
///
/// `edits` are in ascending order and disjoint, expressed in the coordinate space of the
/// document *as it is when the transaction is applied*.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transaction {
    pub edits: Vec<Edit>,
    pub selections_before: SelectionSet,
    pub selections_after: SelectionSet,
}

impl Transaction {
    pub fn new(edits: Vec<Edit>) -> Self {
        Self {
            edits,
            ..Default::default()
        }
    }

    pub fn with_selections(mut self, before: SelectionSet, after: SelectionSet) -> Self {
        self.selections_before = before;
        self.selections_after = after;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn validate(&self, text: &Text) -> Result<(), EditError> {
        let len = text.len_chars();
        let mut prev_end: Option<CharOffset> = None;
        for edit in &self.edits {
            let (start, end) = (edit.range.start, edit.range.end);
            if start > end {
                return Err(EditError::InvertedRange { start, end });
            }
            if end > len {
                return Err(EditError::OutOfBounds { start, end, len });
            }
            if let Some(prev) = prev_end {
                // Touching ranges are fine; overlapping ones are not, and an unsorted list
                // would make the apply loop's running delta meaningless.
                if start < prev {
                    return Err(EditError::Overlapping { at: start });
                }
            }
            prev_end = Some(end);
        }
        Ok(())
    }

    /// Applies the transaction and returns the inverse transaction, whose edits are expressed
    /// in the coordinate space of the *resulting* document.
    pub fn apply(&self, text: &mut Text) -> Result<Transaction, EditError> {
        self.validate(text)?;

        let mut inverse = Vec::with_capacity(self.edits.len());
        // Applying ascending shifts every later range, so carry the running delta rather than
        // re-deriving offsets.
        let mut delta: isize = 0;

        for edit in &self.edits {
            let start = edit.range.start.shift(delta);
            let end = edit.range.end.shift(delta);

            let removed = text.slice_to_string(start..end);
            text.replace(start..end, &edit.text);

            let inserted_len = edit.new_len();
            inverse.push(Edit::replace(
                start..CharOffset(start.get() + inserted_len),
                removed,
            ));
            delta += edit.delta();
        }

        Ok(Transaction {
            edits: inverse,
            selections_before: self.selections_after.clone(),
            selections_after: self.selections_before.clone(),
        })
    }
}

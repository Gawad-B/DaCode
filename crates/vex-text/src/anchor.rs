//! Anchors — positions that survive edits.
//!
//! Marks, selections, diagnostics, folds and every background result are stored as anchors.
//! Without them, an LSP diagnostic computed against version 1281 would point at the wrong
//! character once version 1284 exists. `bias` decides which side an anchor falls on when text
//! is inserted exactly at it, or when the text it points into is deleted.

use crate::coords::CharOffset;
use crate::edit::Edit;

/// Which side of an edit an anchor sticks to.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub enum Bias {
    /// Stays before text inserted at this exact position. Correct for the *end* of a range.
    #[default]
    Left,
    /// Moves after text inserted at this exact position. Correct for the *start* of a range,
    /// and for a cursor, which should end up after what you just typed.
    Right,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub offset: CharOffset,
    pub bias: Bias,
}

impl Anchor {
    pub fn new(offset: CharOffset, bias: Bias) -> Self {
        Self { offset, bias }
    }

    pub fn left(offset: CharOffset) -> Self {
        Self::new(offset, Bias::Left)
    }

    pub fn right(offset: CharOffset) -> Self {
        Self::new(offset, Bias::Right)
    }

    /// Maps this anchor through the edits of a transaction (ascending, disjoint, in the
    /// pre-edit coordinate space).
    pub fn transform(self, edits: &[Edit]) -> Anchor {
        Anchor {
            offset: transform_offset(self.offset, self.bias, edits),
            bias: self.bias,
        }
    }
}

/// The single source of truth for position mapping. Every other transform (selections,
/// diagnostics, folds) delegates here so there is exactly one place this can be wrong.
pub fn transform_offset(offset: CharOffset, bias: Bias, edits: &[Edit]) -> CharOffset {
    let pos = offset.get();
    let mut delta: isize = 0;

    for edit in edits {
        let start = edit.range.start.get();
        let end = edit.range.end.get();

        if pos < start {
            // Edits are ascending, so nothing further can affect this position.
            break;
        }

        if start == end && pos == start {
            // Pure insertion exactly at the anchor: this is the case `bias` exists for.
            // Left stays put (text lands after it); Right slides past the inserted text.
            if bias == Bias::Right {
                delta += edit.delta();
            }
            continue;
        }

        if pos >= end {
            // Entirely after the edit — including `pos == end`, where the anchor lands at the
            // end of the replacement.
            delta += edit.delta();
            continue;
        }

        // start <= pos < end: the anchor sits inside the replaced region, or exactly at an
        // insertion point (start == end is excluded by `pos >= end` above).
        return match bias {
            Bias::Left => CharOffset(start.saturating_add_signed(delta)),
            Bias::Right => CharOffset(start.saturating_add_signed(delta) + edit.new_len()),
        };
    }

    CharOffset(pos.saturating_add_signed(delta))
}

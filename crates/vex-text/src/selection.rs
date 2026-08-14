//! Selections. A cursor **is** a selection — there is no separate cursor type.
//!
//! The document always carries a *set* of selections with one primary. This is the Zed/Helix
//! data model, chosen so that multi-cursor is never a special case: an operator maps over the
//! set, and a single cursor is simply a set of one. Vim's grammar sits on top — an operator
//! resolves its motion into a range per selection.

use crate::anchor::{Bias, transform_offset};
use crate::coords::{CharOffset, Point};
use crate::edit::Edit;
use crate::text::Text;
use smallvec::{SmallVec, smallvec};
use std::ops::Range;

/// A directional range. `anchor` is the fixed end, `head` is the end that moves with motions.
/// `anchor == head` is a caret.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: CharOffset,
    pub head: CharOffset,
}

impl Selection {
    pub fn caret(at: CharOffset) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    pub fn new(anchor: CharOffset, head: CharOffset) -> Self {
        Self { anchor, head }
    }

    pub fn start(&self) -> CharOffset {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> CharOffset {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<CharOffset> {
        self.start()..self.end()
    }

    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }

    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// Length in chars. Zero for a caret.
    pub fn len(&self) -> usize {
        self.end().get() - self.start().get()
    }

    /// A caret selects nothing. Distinct from [`SelectionSet::is_empty`], which is about how
    /// many selections exist, not how much text one covers.
    pub fn is_empty(&self) -> bool {
        self.is_caret()
    }

    /// Moves the head, keeping the anchor — the primitive that visual mode is built from.
    pub fn extend_to(self, head: CharOffset) -> Self {
        Self {
            anchor: self.anchor,
            head,
        }
    }

    pub fn collapse_to_head(self) -> Self {
        Self::caret(self.head)
    }

    pub fn clamp(self, text: &Text) -> Self {
        Self {
            anchor: text.clamp(self.anchor),
            head: text.clamp(self.head),
        }
    }

    /// Maps through edits. The start biases right and the end biases left, so a selection
    /// does not swallow text typed immediately outside it.
    pub fn transform(self, edits: &[Edit]) -> Self {
        let reversed = self.is_reversed();
        let start = transform_offset(self.start(), Bias::Right, edits);
        let end = transform_offset(self.end(), Bias::Left, edits);
        // A collapsed-to-nothing range can invert; keep it a caret instead.
        let (start, end) = if start > end {
            (start, start)
        } else {
            (start, end)
        };
        if reversed {
            Self::new(end, start)
        } else {
            Self::new(start, end)
        }
    }
}

/// The set of live selections. Invariant: non-empty, sorted by start, non-overlapping, and
/// `primary` is a valid index. Use [`SelectionSet::normalize`] after any mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSet {
    ranges: SmallVec<[Selection; 1]>,
    primary: usize,
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self {
            ranges: smallvec![Selection::caret(CharOffset::ZERO)],
            primary: 0,
        }
    }
}

impl SelectionSet {
    pub fn single(selection: Selection) -> Self {
        Self {
            ranges: smallvec![selection],
            primary: 0,
        }
    }

    pub fn from_iter(iter: impl IntoIterator<Item = Selection>, primary: usize) -> Self {
        let ranges: SmallVec<[Selection; 1]> = iter.into_iter().collect();
        if ranges.is_empty() {
            return Self::default();
        }
        let mut set = Self {
            primary: primary.min(ranges.len() - 1),
            ranges,
        };
        set.normalize();
        set
    }

    /// A rectangular selection: one selection per line, all covering the same columns.
    ///
    /// The gesture every editor calls column or block selection, and the one thing the multi-cursor
    /// model here makes almost free — a rectangle *is* a set of selections that happen to line up,
    /// so nothing downstream needs to know it was made this way. Typing, deleting and every
    /// operator already map over the set.
    ///
    /// **Lines too short for the block still take part**, as a caret clamped to their end. That is
    /// what makes typing down the right-hand edge of ragged text work, which is most of why anyone
    /// draws a rectangle in the first place; skipping them would silently drop lines out of an
    /// edit the user believes covers everything they dragged over.
    ///
    /// Direction is preserved, so a block drawn right-to-left has its heads on the left and
    /// `shift`-extending it afterwards grows the side the pointer was on.
    pub fn column(text: &Text, anchor: Point, head: Point) -> Self {
        let (top, bottom) = (anchor.line.min(head.line), anchor.line.max(head.line));
        let (left, right) = (
            anchor.column.min(head.column),
            anchor.column.max(head.column),
        );
        let reversed = head.column < anchor.column;

        let last = text.last_line();
        let mut selections: SmallVec<[Selection; 1]> = SmallVec::new();
        for line in top..=bottom.min(last) {
            let len = text.line_len(line);
            let (start, end) = (left.min(len), right.min(len));
            let (from, to) = if reversed { (end, start) } else { (start, end) };
            selections.push(Selection::new(
                text.point_to_char(Point::new(line, from)),
                text.point_to_char(Point::new(line, to)),
            ));
        }
        if selections.is_empty() {
            return Self::default();
        }

        // The primary is the line the pointer is on, so the statusline reports where you are
        // rather than where the block happens to start.
        let primary = head.line.clamp(top, bottom).saturating_sub(top);
        Self::from_iter(selections, primary)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.ranges.iter()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        false // the invariant guarantees at least one selection
    }

    pub fn primary(&self) -> &Selection {
        &self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    pub fn set_primary(&mut self, index: usize) {
        self.primary = index.min(self.ranges.len() - 1);
    }

    /// Rewrites every selection, then restores the invariant. The primary is tracked by
    /// identity across the sort so it does not jump to a different cursor.
    pub fn map(&self, f: impl FnMut(Selection) -> Selection) -> Self {
        let ranges: SmallVec<[Selection; 1]> = self.ranges.iter().copied().map(f).collect();
        let mut set = Self {
            ranges,
            primary: self.primary,
        };
        set.normalize();
        set
    }

    pub fn transform(&self, edits: &[Edit]) -> Self {
        self.map(|s| s.transform(edits))
    }

    pub fn clamp(&self, text: &Text) -> Self {
        self.map(|s| s.clamp(text))
    }

    /// Sorts by start and merges overlaps, keeping the primary pointing at the selection that
    /// absorbed the old primary.
    pub fn normalize(&mut self) {
        let primary_marker = self.ranges[self.primary];

        let mut order: Vec<usize> = (0..self.ranges.len()).collect();
        order.sort_by_key(|&i| (self.ranges[i].start(), self.ranges[i].end()));
        let sorted: SmallVec<[Selection; 1]> = order.iter().map(|&i| self.ranges[i]).collect();

        let mut merged: SmallVec<[Selection; 1]> = SmallVec::new();
        let mut primary = 0usize;

        for sel in sorted {
            let overlaps = merged.last().is_some_and(|last: &Selection| {
                // Carets that merely touch a range's edge stay separate; overlapping bodies
                // merge, as do two carets at the same offset.
                if sel.is_caret() && last.is_caret() {
                    sel.start() == last.start()
                } else {
                    sel.start() < last.end()
                }
            });

            if overlaps {
                let last_index = merged.len() - 1;
                let last = merged[last_index];
                let start = last.start().min(sel.start());
                let end = last.end().max(sel.end());
                // Merged selections inherit the direction of the one they grew from.
                merged[last_index] = if last.is_reversed() {
                    Selection::new(end, start)
                } else {
                    Selection::new(start, end)
                };
                if sel == primary_marker {
                    primary = last_index;
                }
            } else {
                if sel == primary_marker {
                    primary = merged.len();
                }
                merged.push(sel);
            }
        }

        self.ranges = merged;
        self.primary = primary.min(self.ranges.len() - 1);
    }

    /// The edits implied by replacing every selection with the same text — the shape of a
    /// multi-cursor insertion. Ascending and disjoint, so it is a valid transaction.
    pub fn replace_edits(&self, text: &str) -> Vec<Edit> {
        self.ranges
            .iter()
            .map(|s| Edit::replace(s.range(), text))
            .collect()
    }
}

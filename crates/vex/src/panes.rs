//! Which panes exist, which has focus, and where each is looking.
//!
//! A flat list divided along one axis, not a tree. A tree buys arbitrary nesting that few people
//! arrange deliberately, and charges for it by making every layout and focus question recursive.
//! Splitting three ways and closing the middle one is the behavior that actually gets used, and a
//! list does that exactly.
//!
//! The bookkeeping lives here, away from the event loop, because index arithmetic that is only
//! reachable by pressing keys in a window is index arithmetic that never gets tested.

use vex_display::ScrollAnchor;
use vex_text::{CharOffset, Selection, SelectionSet};

/// How the panes divide the editor area.
pub use crate::chrome::PaneLayout;

/// One pane's view: which buffer, and where in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    /// Buffer index, in the same numbering the buffer palette uses.
    pub buffer: usize,
    pub scroll: ScrollAnchor,
    pub selections: SelectionSet,
}

impl Pane {
    pub fn new(buffer: usize) -> Self {
        Self {
            buffer,
            scroll: ScrollAnchor::default(),
            selections: SelectionSet::single(Selection::caret(CharOffset(0))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Panes {
    list: Vec<Pane>,
    active: usize,
    pub layout: PaneLayout,
}

impl Default for Panes {
    fn default() -> Self {
        Self::new()
    }
}

impl Panes {
    pub fn new() -> Self {
        Self {
            list: vec![Pane::new(0)],
            active: 0,
            layout: PaneLayout::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_split(&self) -> bool {
        self.list.len() > 1
    }

    /// Always false — there is always at least one pane. Present because `len` without `is_empty`
    /// is a lint, and answering it honestly is better than deleting `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn iter(&self) -> impl Iterator<Item = &Pane> {
        self.list.iter()
    }

    pub fn focused(&self) -> &Pane {
        // The invariant this type exists to keep: there is always a pane, and `active` always names
        // one of them.
        &self.list[self.active]
    }

    pub fn focused_mut(&mut self) -> &mut Pane {
        &mut self.list[self.active]
    }

    /// Adds a pane beside the focused one, showing the same thing, and moves focus into it.
    ///
    /// The same thing on purpose: a split is as often used to compare two parts of one file as to
    /// hold two files, and starting from where you were is the shorter path to either.
    pub fn split(&mut self, layout: PaneLayout) {
        let twin = self.list[self.active].clone();
        self.list.insert(self.active + 1, twin);
        self.active += 1;
        self.layout = layout;
    }

    /// Closes the focused pane. The last one never closes.
    ///
    /// Returns whether anything closed, so the caller can leave the editor alone when it did not —
    /// an editor with no viewport is not a state worth being able to reach by holding a key.
    pub fn close(&mut self) -> bool {
        if self.list.len() == 1 {
            return false;
        }
        self.list.remove(self.active);
        self.active = self.active.min(self.list.len() - 1);
        true
    }

    /// Moves focus to the next or previous pane, wrapping.
    ///
    /// Wrapping because with two panes `ctrl-w w` twice should bring you home, rather than stopping
    /// at an edge you cannot see.
    pub fn cycle(&mut self, forward: bool) -> usize {
        let count = self.list.len();
        self.active = if forward {
            (self.active + 1) % count
        } else {
            (self.active + count - 1) % count
        };
        self.active
    }

    /// Points the focused pane at the buffer that just became active.
    ///
    /// Opening a file changes which buffer is live, and a pane that does not follow keeps drawing the
    /// one it remembers. That is not a subtle failure: it showed the *previous* buffer's text — an
    /// empty `untitled` one, right after opening a folder — so choosing a file from the tree appeared
    /// to open an empty file.
    pub fn follow(&mut self, active: usize) {
        self.list[self.active].buffer = active;
    }

    /// Repairs pane buffer indices after a buffer is removed from the list.
    ///
    /// Buffers are addressed by position, so closing one shifts every index above it. Without this a
    /// pane would quietly start showing its neighbour's file — the kind of bug that looks like the
    /// editor lost your work.
    pub fn buffer_closed(&mut self, closed: usize) {
        for pane in &mut self.list {
            if pane.buffer > closed {
                pane.buffer -= 1;
            } else if pane.buffer == closed {
                // Whatever took its place at that index, clamped to what exists.
                pane.buffer = closed.saturating_sub(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_duplicates_the_focused_pane_and_focuses_the_new_one() {
        let mut panes = Panes::new();
        panes.focused_mut().buffer = 3;
        panes.focused_mut().scroll = ScrollAnchor {
            line: 40,
            sub_row: 1,
        };

        panes.split(PaneLayout::Vertical);

        assert_eq!(panes.len(), 2);
        assert_eq!(panes.active(), 1, "focus follows the split");
        assert_eq!(
            panes.focused().buffer,
            3,
            "showing what you were looking at"
        );
        assert_eq!(
            panes.focused().scroll.line,
            40,
            "and at the same place in it"
        );
        assert_eq!(panes.layout, PaneLayout::Vertical);
    }

    #[test]
    fn the_last_pane_never_closes() {
        let mut panes = Panes::new();
        assert!(!panes.close());
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn closing_the_last_pane_in_the_list_moves_focus_back() {
        let mut panes = Panes::new();
        panes.split(PaneLayout::Vertical);
        panes.split(PaneLayout::Vertical);
        assert_eq!((panes.len(), panes.active()), (3, 2));

        assert!(panes.close());
        assert_eq!(
            (panes.len(), panes.active()),
            (2, 1),
            "focus cannot be left past the end"
        );
    }

    #[test]
    fn focus_wraps_in_both_directions() {
        let mut panes = Panes::new();
        panes.split(PaneLayout::Vertical);
        panes.split(PaneLayout::Vertical);

        assert_eq!(panes.cycle(true), 0, "past the end wraps to the start");
        assert_eq!(panes.cycle(false), 2, "before the start wraps to the end");
        assert_eq!(panes.cycle(false), 1);
    }

    #[test]
    fn closing_a_buffer_renumbers_the_panes_that_pointed_past_it() {
        // Buffers are addressed by position, so removing one shifts everything above it. A pane that
        // does not follow starts showing a different file than the one it was opened on.
        let mut panes = Panes::new();
        panes.split(PaneLayout::Vertical);
        panes.split(PaneLayout::Vertical);
        panes.list[0].buffer = 0;
        panes.list[1].buffer = 2;
        panes.list[2].buffer = 3;

        panes.buffer_closed(1);

        let buffers: Vec<usize> = panes.iter().map(|pane| pane.buffer).collect();
        assert_eq!(
            buffers,
            vec![0, 1, 2],
            "panes above the closed buffer shift down, those below stay"
        );
    }

    #[test]
    fn a_pane_showing_the_closed_buffer_falls_back_rather_than_dangling() {
        let mut panes = Panes::new();
        panes.split(PaneLayout::Vertical);
        panes.list[0].buffer = 1;
        panes.list[1].buffer = 1;

        panes.buffer_closed(1);

        assert!(
            panes.iter().all(|pane| pane.buffer == 0),
            "a pane pointing at a closed buffer must land on one that exists"
        );
    }

    #[test]
    fn there_is_always_exactly_one_focused_pane() {
        let mut panes = Panes::new();
        for _ in 0..4 {
            panes.split(PaneLayout::Horizontal);
        }
        for _ in 0..10 {
            panes.cycle(true);
            assert!(panes.active() < panes.len());
        }
        while panes.close() {
            assert!(panes.active() < panes.len());
        }
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn only_the_focused_pane_follows_the_active_buffer() {
        // The regression this method exists for: opening a file from the tree drew the buffer the
        // pane still remembered, which right after opening a folder was an empty untitled one.
        let mut panes = Panes::new();
        panes.split(PaneLayout::Vertical);
        panes.list[0].buffer = 0;
        panes.list[1].buffer = 0;
        assert_eq!(panes.active(), 1);

        panes.follow(4);

        assert_eq!(
            panes.focused().buffer,
            4,
            "the focused pane shows what is live"
        );
        assert_eq!(
            panes.list[0].buffer, 0,
            "the other pane keeps looking at what it was looking at"
        );
    }
}

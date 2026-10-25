//! `dacode-text` — text storage, coordinate spaces, edits, anchors and selections.
//!
//! The bottom layer of the editor. It knows nothing about rendering, input or services, and
//! it is where the two decisions that cannot be retrofitted live:
//!
//! 1. **Typed coordinate spaces** ([`ByteOffset`], [`CharOffset`], [`Point`],
//!    [`DisplayPoint`], [`Utf16Column`]) — never bare integers, because tree-sitter, LSP,
//!    the rope and the renderer all disagree about what a position is.
//! 2. **One mutation path** ([`Transaction`]) — so undo, multi-cursor, dot-repeat and
//!    stale-result invalidation all derive from the same primitive.

pub mod anchor;
pub mod coords;
pub mod edit;
pub mod selection;
pub mod text;

pub use anchor::{Anchor, Bias, transform_offset};
pub use coords::{ByteOffset, CharOffset, DisplayPoint, Point, Utf16Column};
pub use edit::{Edit, EditError, Transaction};
pub use selection::{Selection, SelectionSet};
pub use text::Text;

/// Monotonic document version. Every background result (syntax tree, diagnostics, search
/// hits) is tagged with the version it was computed against; results whose version is behind
/// the buffer's are **discarded, never applied**. This is the mechanism behind the editor's
/// "nothing on the UI thread is unbounded" contract, and the design surfaces it directly in
/// the structure sidebar's async ledger.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u64);

impl Version {
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub fn is_fresh_for(self, buffer: Version) -> bool {
        self == buffer
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

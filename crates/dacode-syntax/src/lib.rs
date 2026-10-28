//! `dacode-syntax` — incremental syntax highlighting via tree-sitter.
//!
//! Two things make this more than "call a parser":
//!
//! 1. **Incremental reparse.** The tree is edited with the same byte ranges the document was
//!    edited with, so a keystroke reparses the changed subtree rather than the file. Without
//!    this, typing in a large file blocks the UI thread and the 60fps contract is dead.
//! 2. **Version tagging.** Every result carries the document [`Version`] it was computed
//!    against. Anything behind the buffer's version is **discarded, never applied** — the rule
//!    from §1.1 of the plan, and what the editor's async ledger displays.
//!
//! Highlight captures are collapsed into the theme's small set of roles rather than being
//! carried through as strings. A theme with fifty capture colors is one nobody can keep
//! coherent, and the mapping belongs here, next to the queries.

pub mod highlight;
pub mod structure;
pub mod worker;

pub use highlight::{Comment, Highlight, HighlightSpan, Highlighter, Language};
pub use structure::{ObjectKind, Structure};
pub use worker::{SyntaxRequest, SyntaxResult, SyntaxWorker};

use dacode_text::Version;

/// A parsed result plus the version it was computed against.
///
/// The pairing is the point: a result without its version cannot be checked for staleness, and
/// an unchecked stale result is a highlight painted over text that has since moved.
#[derive(Debug, Clone)]
pub struct Versioned<T> {
    pub version: Version,
    pub value: T,
}

impl<T> Versioned<T> {
    pub fn new(version: Version, value: T) -> Self {
        Self { version, value }
    }

    pub fn is_fresh_for(&self, buffer: Version) -> bool {
        self.version == buffer
    }
}

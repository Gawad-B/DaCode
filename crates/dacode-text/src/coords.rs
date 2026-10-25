//! Coordinate spaces.
//!
//! Editors are bug factories because "position" is ambiguous: tree-sitter speaks bytes, LSP
//! speaks UTF-16 code units, ropey prefers chars, and rendering needs post-wrap rows. These
//! are distinct newtypes on purpose — they do not implicitly convert into one another. Every
//! conversion goes through [`crate::Text`], which is the only thing that knows the content.

use std::fmt;

macro_rules! offset_newtype {
    ($(#[$m:meta])* $name:ident, $unit:literal) => {
        $(#[$m])*
        #[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub usize);

        impl $name {
            pub const ZERO: Self = Self(0);

            #[inline]
            pub fn get(self) -> usize {
                self.0
            }

            /// Saturating offset by a signed delta. Offsets are never negative.
            #[inline]
            pub fn shift(self, delta: isize) -> Self {
                Self(self.0.saturating_add_signed(delta))
            }
        }

        impl From<usize> for $name {
            #[inline]
            fn from(v: usize) -> Self {
                Self(v)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", self.0, $unit)
            }
        }
    };
}

offset_newtype!(
    /// Offset in UTF-8 bytes from the start of the document. The coordinate space of
    /// tree-sitter and of bytes on disk.
    ByteOffset,
    "b"
);

offset_newtype!(
    /// Offset in Unicode scalar values (`char`) from the start of the document. The native
    /// coordinate space of the rope, and the canonical internal position type.
    CharOffset,
    "c"
);

offset_newtype!(
    /// Offset in UTF-16 code units, **relative to the start of a line**. Exists solely
    /// because that is what the Language Server Protocol uses for `Position.character`.
    Utf16Column,
    "u16"
);

/// A logical position: a line, and a column measured in `char`s into that line.
///
/// Independent of how the line is displayed — soft wrap, folds, tab width and inlay hints do
/// not affect it. Contrast with [`DisplayPoint`].
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: usize,
    pub column: usize,
}

impl Point {
    pub const ZERO: Self = Self { line: 0, column: 0 };

    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

impl fmt::Debug for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A position on screen: a visual row, and a column in cells.
///
/// One buffer line may span many display rows (soft wrap), and many buffer lines may collapse
/// into none (folds), so this is *not* interchangeable with [`Point`]. Kept as a separate type
/// from the beginning because soft wrap is a v1 feature and retrofitting wrap-awareness into
/// viewport math is one of the expensive mistakes this design avoids.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayPoint {
    pub row: usize,
    pub column: usize,
}

impl DisplayPoint {
    pub const ZERO: Self = Self { row: 0, column: 0 };

    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

impl fmt::Debug for DisplayPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "row {} col {}", self.row, self.column)
    }
}

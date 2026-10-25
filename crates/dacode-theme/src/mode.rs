//! Editor modes, and the visual state each one implies.
//!
//! "Mode is the environment": the mode is not a word in a corner, it is the hue every accent
//! on screen derives from. This type is the single state variable that drives that, and it is
//! deliberately owned by the theme layer so no widget invents its own mode colors.

/// Cursor geometry per mode, in multiples of the code line height / cell width.
///
/// Shape must communicate the mode *before* color does — that survives colorblindness and
/// low-light screens. The renderer treats these as guidance, not exact pixels.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum CursorShape {
    /// Wide translucent block with a solid underline. Never blinks — a resting cursor that
    /// blinks in normal mode reads as "waiting for input" when it is actually waiting for a
    /// command.
    Block { fill_alpha: f32, underline: bool },
    /// Thin blinking bar.
    Bar { width_px: f32, blink: bool },
    /// Solid-filled block.
    Solid { fill_alpha: f32 },
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    Replace,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Normal, Mode::Insert, Mode::Visual, Mode::Replace];

    /// The mode's hue in degrees. Lightness and chroma come from the theme, so all four modes
    /// stay at equal perceptual weight.
    pub const fn hue(self) -> f32 {
        match self {
            Mode::Normal => 250.0,
            Mode::Insert => 155.0,
            Mode::Visual => 70.0,
            Mode::Replace => 25.0,
        }
    }

    /// Uppercase name for the statusline and title bar.
    pub const fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::Replace => "REPLACE",
        }
    }

    pub const fn cursor(self) -> CursorShape {
        match self {
            Mode::Normal => CursorShape::Block {
                fill_alpha: 0.30,
                underline: true,
            },
            Mode::Insert => CursorShape::Bar {
                width_px: 2.0,
                blink: true,
            },
            // In visual mode the *range* carries the signal; the primary cursor keeps the
            // normal block so you can still see which end moves.
            Mode::Visual => CursorShape::Block {
                fill_alpha: 0.30,
                underline: true,
            },
            Mode::Replace => CursorShape::Solid { fill_alpha: 0.50 },
        }
    }

    /// Alpha of the current-line tint. Visual mode is stronger because the selection fill and
    /// the line tint share a hue and would otherwise be indistinguishable.
    pub const fn line_tint_alpha(self) -> f32 {
        match self {
            Mode::Visual => 0.14,
            _ => 0.06,
        }
    }
}

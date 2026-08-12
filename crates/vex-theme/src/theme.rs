//! Theme tokens, dark and light.
//!
//! Values follow the design handoff. Treated as a *palette and a set of relationships*, not as
//! a stylesheet to reproduce: the renderer decides geometry, this decides color.

use crate::color::Oklch;
use crate::mode::Mode;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

/// Colors for tree-sitter highlight captures. Kept small on purpose: a theme with fifty
/// capture colors is a theme nobody can keep coherent. Highlight queries map many captures
/// onto these few roles.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SyntaxPalette {
    pub keyword: Oklch,
    pub type_: Oklch,
    pub function: Oklch,
    pub attribute: Oklch,
    /// Set in *italic serif*, larger than the code around it — the design's most distinctive
    /// move, and the reason comments read as prose rather than as disabled code.
    pub comment: Oklch,
    /// The handoff specifies four syntax colors; real highlight queries also produce strings,
    /// numbers and constants. These extend the same family — the siblings' lightness and chroma,
    /// new hues — rather than introducing a second palette.
    pub string: Oklch,
    pub number: Oklch,
    pub constant: Oklch,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Theme {
    pub appearance: Appearance,

    // Surfaces
    pub bg: Oklch,
    pub bg_alt: Oklch,
    pub bg_panel: Oklch,
    pub row_active: Oklch,

    // Lines
    pub rule: Oklch,
    pub rule_soft: Oklch,

    // Text
    pub ink: Oklch,
    pub ink_dim: Oklch,
    pub ink_faint: Oklch,

    // Overlays
    pub scrim: Oklch,
    pub shadow: Oklch,

    pub syntax: SyntaxPalette,

    /// Lightness and chroma shared by all four mode accents. Moving lightness between themes
    /// (0.74 dark, 0.52 light) is what keeps accents legible on paper without changing hue.
    mode_l: f32,
    mode_c: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            bg: Oklch::new(0.165, 0.008, 70.0),
            bg_alt: Oklch::new(0.185, 0.009, 70.0),
            bg_panel: Oklch::new(0.200, 0.009, 70.0),
            row_active: Oklch::new(0.250, 0.011, 70.0),
            rule: Oklch::new(0.300, 0.010, 70.0),
            rule_soft: Oklch::new(0.250, 0.009, 70.0),
            ink: Oklch::new(0.900, 0.012, 80.0),
            ink_dim: Oklch::new(0.680, 0.012, 75.0),
            ink_faint: Oklch::new(0.520, 0.010, 72.0),
            scrim: Oklch::new(0.120, 0.006, 70.0).with_alpha(0.62),
            shadow: Oklch::new(0.050, 0.000, 0.0).with_alpha(0.60),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.74, 0.12, 300.0),
                type_: Oklch::new(0.83, 0.09, 60.0),
                function: Oklch::new(0.82, 0.09, 215.0),
                attribute: Oklch::new(0.66, 0.09, 300.0),
                comment: Oklch::new(0.60, 0.02, 60.0),
                string: Oklch::new(0.80, 0.09, 150.0),
                number: Oklch::new(0.82, 0.09, 30.0),
                constant: Oklch::new(0.78, 0.09, 330.0),
            },
            mode_l: 0.74,
            mode_c: 0.13,
        }
    }

    pub const fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            bg: Oklch::new(0.965, 0.008, 85.0),
            bg_alt: Oklch::new(0.975, 0.008, 85.0),
            bg_panel: Oklch::new(0.945, 0.010, 85.0),
            row_active: Oklch::new(0.955, 0.010, 85.0),
            rule: Oklch::new(0.880, 0.012, 85.0),
            rule_soft: Oklch::new(0.910, 0.010, 85.0),
            ink: Oklch::new(0.260, 0.015, 70.0),
            ink_dim: Oklch::new(0.500, 0.012, 70.0),
            ink_faint: Oklch::new(0.680, 0.012, 70.0),
            scrim: Oklch::new(0.800, 0.010, 85.0).with_alpha(0.55),
            shadow: Oklch::new(0.500, 0.010, 70.0).with_alpha(0.22),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.42, 0.13, 300.0),
                type_: Oklch::new(0.44, 0.11, 45.0),
                function: Oklch::new(0.45, 0.10, 230.0),
                attribute: Oklch::new(0.50, 0.10, 300.0),
                comment: Oklch::new(0.52, 0.02, 60.0),
                string: Oklch::new(0.44, 0.11, 150.0),
                number: Oklch::new(0.45, 0.11, 30.0),
                constant: Oklch::new(0.44, 0.11, 330.0),
            },
            mode_l: 0.52,
            mode_c: 0.13,
        }
    }

    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(),
            Appearance::Light => Self::light(),
        }
    }

    // ---- mode-derived colors -----------------------------------------------------------
    //
    // Everything mode-colored on screen comes from `accent`. Nothing downstream is allowed to
    // hardcode a mode hue, which is what keeps a mode change coherent across the whole window.

    pub fn accent(&self, mode: Mode) -> Oklch {
        Oklch::new(self.mode_l, self.mode_c, mode.hue())
    }

    /// Tint behind the cursor's line. Resolved against the canvas so the renderer can emit it
    /// as an opaque quad.
    pub fn line_tint(&self, mode: Mode) -> [f32; 4] {
        self.accent(mode)
            .with_alpha(mode.line_tint_alpha())
            .over(self.bg)
    }

    /// The gutter's mode wash, as `(transparent_edge, tinted_edge)` for a horizontal gradient.
    pub fn gutter_tint(&self, mode: Mode) -> ([f32; 4], [f32; 4]) {
        (
            self.bg.to_linear(),
            self.accent(mode).with_alpha(0.06).over(self.bg),
        )
    }

    /// Selection fill in visual mode.
    pub fn selection_fill(&self, mode: Mode) -> [f32; 4] {
        self.accent(mode).with_alpha(0.14).over(self.bg)
    }

    // ---- instrumentation ---------------------------------------------------------------
    //
    // The async ledger reuses the insert and visual hues rather than introducing a semantic
    // green/amber pair, so status stays inside the same four-hue palette.

    /// A background result computed against the current buffer version.
    pub fn fresh(&self) -> Oklch {
        self.accent(Mode::Insert)
    }

    /// A background result that is behind the buffer — displayed, then discarded, never applied.
    pub fn stale(&self) -> Oklch {
        self.accent(Mode::Visual)
    }

    /// Over-budget frame time. Reuses the replace hue for the same reason.
    pub fn over_budget(&self) -> Oklch {
        self.accent(Mode::Replace)
    }
}

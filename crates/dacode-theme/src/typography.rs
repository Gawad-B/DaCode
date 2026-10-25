//! Type roles and metrics.
//!
//! One face — Fira Code — at a handful of sizes and weights. Roles are named by intent so a widget
//! asks for `Role::Label` rather than picking a size, and so the type scale can move as one.

/// The one face the editor draws everything in, then what to fall back to if it is missing.
///
/// Fira Code, for the interface as well as the code. The design once set headings and comments in a
/// serif and labels in a sans, and the contrast was the idea; what people wanted was one familiar
/// face, everywhere they look, and a single monospaced face also makes every width estimate in the
/// chrome exact rather than an average. The fallbacks are monospaced too, so a machine without Fira
/// Code still lays text out on the same grid.
pub const FACES: &[&str] = &[
    "Fira Code",
    "JetBrains Mono",
    "DejaVu Sans Mono",
    "monospace",
];

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Style {
    Roman,
    Italic,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Role {
    /// Window title, panel headings, the wordmark.
    Display,
    /// Body prose in panels.
    Prose,
    /// Buffer text.
    Code,
    /// Comments inside code: italic serif, slightly larger than the code around it so the
    /// optical size matches despite the different family.
    Comment,
    /// Gutter line numbers — serif figures.
    LineNumber,
    /// UI values in panels and the statusline.
    Value,
    /// Uppercase tracked labels.
    Label,
    /// The interface's own words — tab names, the file tree, buttons — in the sans, as people
    /// expect a desktop application's chrome to be set.
    Ui,
    /// The product name in the title bar.
    Brand,
}

/// A resolved font request.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FontSpec {
    pub style: Style,
    pub weight: u16,
    pub size_px: f32,
    pub line_height: f32,
    /// Letter spacing in em. Non-zero only for labels.
    pub tracking_em: f32,
    pub uppercase: bool,
}

/// Type scale, derived from the code size so changing font size scales the whole UI coherently.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Typography {
    /// Code size in px. Configurable 11–18.
    pub code_size: f32,
    /// Multiple of the code size. The design's generous 1.9 is what makes serif comments and
    /// mono code coexist without the lines fighting.
    pub code_line_height: f32,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            code_size: 13.5,
            code_line_height: 1.9,
        }
    }
}

impl Typography {
    pub const MIN_CODE_SIZE: f32 = 11.0;
    pub const MAX_CODE_SIZE: f32 = 18.0;

    pub fn with_code_size(size: f32) -> Self {
        Self {
            code_size: size.clamp(Self::MIN_CODE_SIZE, Self::MAX_CODE_SIZE),
            ..Default::default()
        }
    }

    /// Row height in px, identical for the gutter and the code area. If these ever diverge the
    /// line numbers stop registering with their lines, which is the single most noticeable
    /// layout defect an editor can have.
    pub fn row_height(&self) -> f32 {
        (self.code_size * self.code_line_height).round()
    }

    /// Nominal width of one character in a role, in px.
    ///
    /// Derived from the type scale rather than measured from the shaper — the same approximation
    /// the code grid already makes. It exists so a string can be shortened *before* it is handed to
    /// the renderer: a run is clipped to its rectangle, so text too long for a narrow panel is cut
    /// mid-word with nothing to say that anything is missing. That is what a real run in a small
    /// window showed, and an approximate measure that ends in an ellipsis beats an exact one that
    /// ends in silence.
    ///
    /// Every role is set in one monospaced face, so this is the face's own constant advance — exact,
    /// not an average — plus the tracking labels add.
    pub fn advance(&self, role: Role) -> f32 {
        let spec = self.spec(role);
        spec.size_px * (0.60 + spec.tracking_em)
    }

    pub fn spec(&self, role: Role) -> FontSpec {
        let code = self.code_size;
        match role {
            Role::Display => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: (code * 2.5).round(),
                line_height: 0.9,
                tracking_em: -0.015,
                uppercase: false,
            },
            Role::Prose => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: (code * 1.25).round(),
                line_height: 1.5,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Code => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: code,
                line_height: self.code_line_height,
                tracking_em: 0.0,
                uppercase: false,
            },
            // A comment is code, and it is set as code: the same face, upright, at the same size.
            // It was italic serif a size larger, which is a real typographic idea — prose among
            // instructions — and it is the thing people notice first and like least. It also has to
            // fight the grid: a proportional face in a monospaced column cannot align with the code
            // above it, so a commented-out line sat visibly off the one it was commenting out. The
            // theme still dims it, which is the distinction that was doing the work anyway.
            Role::Comment => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: code,
                line_height: self.code_line_height,
                tracking_em: 0.0,
                uppercase: false,
            },
            // In the code face, a little smaller than the code: figures that line up in a column and
            // recede beside the text they number.
            Role::LineNumber => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: (code * 0.86).round(),
                line_height: self.code_line_height,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Value => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: (code * 0.85).round(),
                line_height: 1.4,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Label => FontSpec {
                style: Style::Roman,
                weight: 500,
                size_px: (code * 0.74).round(),
                line_height: 1.2,
                tracking_em: 0.2,
                uppercase: true,
            },
            Role::Ui => FontSpec {
                style: Style::Roman,
                weight: 400,
                size_px: (code * 0.93).round(),
                line_height: 1.3,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Brand => FontSpec {
                style: Style::Roman,
                weight: 600,
                size_px: (code * 1.15).round(),
                line_height: 1.2,
                tracking_em: 0.01,
                uppercase: false,
            },
        }
    }
}

/// Spacing scale in px. Editor frame padding is `xl`.
pub mod space {
    pub const XXS: f32 = 4.0;
    pub const XS: f32 = 8.0;
    pub const SM: f32 = 12.0;
    pub const MD: f32 = 16.0;
    pub const LG: f32 = 22.0;
    pub const XL: f32 = 34.0;
    pub const XXL: f32 = 56.0;
}

/// Every corner in the design is square and every border is 1px. Kept as named constants so
/// the intent survives contact with the renderer.
pub mod frame {
    pub const RADIUS: f32 = 0.0;
    pub const BORDER: f32 = 1.0;
    /// Active tab underline, active line's left border, current-milestone outline.
    pub const ACCENT_BORDER: f32 = 2.0;
}

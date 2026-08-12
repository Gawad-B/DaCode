//! Type roles and metrics.
//!
//! The design's central typographic idea is a deliberate contrast between *typeset serif prose*
//! and *monospaced code*: comments, gutter figures and headings are serif; code and values are
//! mono; labels are small uppercase sans. Roles are named by intent so a widget asks for
//! `Role::Label` rather than picking a family and a size.

/// Font families the editor bundles. Bundled rather than fetched, so the first frame never
/// waits on a font and the editor works offline.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Family {
    /// Display serif with a true italic — headings, comments, gutter figures, prose.
    Serif,
    /// The code face. Everything monospaced.
    Mono,
    /// Small uppercase labels.
    Sans,
}

impl Family {
    /// Preferred face, then fallbacks. The design names Instrument Serif and IBM Plex; the
    /// fallbacks keep the editor usable before those are bundled, since the *contrast* between
    /// families is the point rather than the specific faces.
    pub const fn candidates(self) -> &'static [&'static str] {
        match self {
            Family::Serif => &["Instrument Serif", "EB Garamond", "Georgia", "serif"],
            Family::Mono => &[
                "IBM Plex Mono",
                "JetBrains Mono",
                "DejaVu Sans Mono",
                "monospace",
            ],
            Family::Sans => &["IBM Plex Sans", "Inter", "DejaVu Sans", "sans-serif"],
        }
    }
}

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
}

/// A resolved font request.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FontSpec {
    pub family: Family,
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

    pub fn spec(&self, role: Role) -> FontSpec {
        let code = self.code_size;
        match role {
            Role::Display => FontSpec {
                family: Family::Serif,
                style: Style::Italic,
                weight: 400,
                size_px: (code * 2.5).round(),
                line_height: 0.9,
                tracking_em: -0.015,
                uppercase: false,
            },
            Role::Prose => FontSpec {
                family: Family::Serif,
                style: Style::Roman,
                weight: 400,
                size_px: (code * 1.25).round(),
                line_height: 1.5,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Code => FontSpec {
                family: Family::Mono,
                style: Style::Roman,
                weight: 400,
                size_px: code,
                line_height: self.code_line_height,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Comment => FontSpec {
                family: Family::Serif,
                style: Style::Italic,
                weight: 400,
                size_px: code * 1.15,
                line_height: self.code_line_height,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::LineNumber => FontSpec {
                family: Family::Serif,
                style: Style::Roman,
                weight: 400,
                size_px: code,
                line_height: self.code_line_height,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Value => FontSpec {
                family: Family::Mono,
                style: Style::Roman,
                weight: 400,
                size_px: (code * 0.85).round(),
                line_height: 1.4,
                tracking_em: 0.0,
                uppercase: false,
            },
            Role::Label => FontSpec {
                family: Family::Sans,
                style: Style::Roman,
                weight: 500,
                size_px: (code * 0.74).round(),
                line_height: 1.2,
                tracking_em: 0.2,
                uppercase: true,
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

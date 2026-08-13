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

/// A built-in theme: the name it is known by, and how to build it.
pub type BuiltIn = (&'static str, fn() -> Theme);

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

    /// Sets one theme token by name, returning false for an unknown name.
    ///
    /// The single place a token name maps to a field, shared by the Lua config, the TOML reader and
    /// the VSCode importer. Three copies of this table would drift, and a token that silently does
    /// nothing in one of the three is indistinguishable to the user from a broken theme.
    ///
    /// Syntax roles are reachable bare (`keyword`) or qualified (`syntax.keyword`); nothing else is
    /// ambiguous between the two.
    pub fn set_token(&mut self, name: &str, color: Oklch) -> bool {
        let target = match name.trim().trim_start_matches("theme.") {
            "bg" | "surfaces.bg" => &mut self.bg,
            "bg_alt" | "surfaces.bg_alt" => &mut self.bg_alt,
            "bg_panel" | "surfaces.bg_panel" => &mut self.bg_panel,
            "row_active" | "surfaces.row_active" => &mut self.row_active,
            "rule" | "lines.rule" => &mut self.rule,
            "rule_soft" | "lines.rule_soft" => &mut self.rule_soft,
            "ink" | "text.ink" => &mut self.ink,
            "ink_dim" | "text.ink_dim" => &mut self.ink_dim,
            "ink_faint" | "text.ink_faint" => &mut self.ink_faint,
            "scrim" | "overlays.scrim" => &mut self.scrim,
            "shadow" | "overlays.shadow" => &mut self.shadow,
            "keyword" | "syntax.keyword" => &mut self.syntax.keyword,
            // `type` is a Rust keyword, so the field is `type_`; the token name is not.
            "type" | "syntax.type" => &mut self.syntax.type_,
            "function" | "syntax.function" => &mut self.syntax.function,
            "attribute" | "syntax.attribute" => &mut self.syntax.attribute,
            "comment" | "syntax.comment" => &mut self.syntax.comment,
            "string" | "syntax.string" => &mut self.syntax.string,
            "number" | "syntax.number" => &mut self.syntax.number,
            "constant" | "syntax.constant" => &mut self.syntax.constant,
            _ => return false,
        };
        *target = color;
        true
    }

    /// Every name [`Theme::set_token`] accepts, for error messages and documentation.
    pub const TOKENS: [&'static str; 19] = [
        "bg",
        "bg_alt",
        "bg_panel",
        "row_active",
        "rule",
        "rule_soft",
        "ink",
        "ink_dim",
        "ink_faint",
        "scrim",
        "shadow",
        "keyword",
        "type",
        "function",
        "attribute",
        "comment",
        "string",
        "number",
        "constant",
    ];

    /// Rosé Pine — the official palette, converted from its hex values to OKLCH.
    ///
    /// Deliberately low-contrast: that is the theme's identity, not an oversight, so the values are
    /// taken as published rather than "corrected" towards this design's usual separation.
    pub const fn rose_pine() -> Self {
        Self {
            appearance: Appearance::Dark,
            bg: Oklch::new(0.2134, 0.0255, 291.1),
            bg_alt: Oklch::new(0.2413, 0.0322, 289.1),
            bg_panel: Oklch::new(0.2708, 0.0423, 289.2),
            row_active: Oklch::new(0.3717, 0.0358, 291.1),
            rule: Oklch::new(0.4399, 0.0392, 290.4),
            rule_soft: Oklch::new(0.2512, 0.0263, 287.6),
            ink: Oklch::new(0.9088, 0.0299, 290.0),
            ink_dim: Oklch::new(0.6539, 0.0444, 291.2),
            ink_faint: Oklch::new(0.5383, 0.0435, 291.4),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.5277, 0.0793, 227.7),
                type_: Oklch::new(0.8219, 0.0543, 209.6),
                function: Oklch::new(0.8363, 0.0544, 21.1),
                attribute: Oklch::new(0.776, 0.0945, 305.0),
                comment: Oklch::new(0.5383, 0.0435, 291.4),
                string: Oklch::new(0.8429, 0.11, 74.6),
                number: Oklch::new(0.776, 0.0945, 305.0),
                constant: Oklch::new(0.6977, 0.1565, 4.2),
            },
            scrim: Oklch::new(0.15, 0.02, 291.0).with_alpha(0.66),
            shadow: Oklch::new(0.05, 0.0, 0.0).with_alpha(0.60),
            mode_l: 0.74,
            mode_c: 0.11,
        }
    }

    /// Rosé Pine Moon — the same palette a shade lighter at the base.
    pub const fn rose_pine_moon() -> Self {
        Self {
            appearance: Appearance::Dark,
            bg: Oklch::new(0.2604, 0.039, 287.7),
            bg_alt: Oklch::new(0.2875, 0.0435, 289.1),
            bg_panel: Oklch::new(0.3463, 0.0502, 289.8),
            row_active: Oklch::new(0.3887, 0.0421, 289.8),
            rule: Oklch::new(0.4535, 0.0454, 291.0),
            rule_soft: Oklch::new(0.2894, 0.0398, 287.8),
            ink: Oklch::new(0.9088, 0.0299, 290.0),
            ink_dim: Oklch::new(0.6539, 0.0444, 291.2),
            ink_faint: Oklch::new(0.5383, 0.0435, 291.4),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.6138, 0.0926, 228.0),
                type_: Oklch::new(0.8219, 0.0543, 209.6),
                function: Oklch::new(0.7651, 0.0967, 21.9),
                attribute: Oklch::new(0.776, 0.0945, 305.0),
                comment: Oklch::new(0.5383, 0.0435, 291.4),
                string: Oklch::new(0.8429, 0.11, 74.6),
                number: Oklch::new(0.776, 0.0945, 305.0),
                constant: Oklch::new(0.6977, 0.1565, 4.2),
            },
            scrim: Oklch::new(0.18, 0.03, 288.0).with_alpha(0.66),
            shadow: Oklch::new(0.05, 0.0, 0.0).with_alpha(0.60),
            mode_l: 0.75,
            mode_c: 0.11,
        }
    }

    /// True black, with high-chroma accents.
    ///
    /// `bg` is exactly #000000, which on an OLED panel means those pixels are *off* — the point of the
    /// theme. The accents carry more chroma than the other themes to hold their own against it.
    pub const fn amoled() -> Self {
        Self {
            appearance: Appearance::Dark,
            bg: Oklch::new(0.0, 0.0, 0.0),
            bg_alt: Oklch::new(0.1286, 0.0, 86.0),
            bg_panel: Oklch::new(0.1591, 0.0, 86.0),
            row_active: Oklch::new(0.2178, 0.0, 86.0),
            rule: Oklch::new(0.285, 0.0, 86.0),
            rule_soft: Oklch::new(0.1957, 0.0, 86.0),
            ink: Oklch::new(1.0, 0.0, 86.0),
            ink_dim: Oklch::new(0.7826, 0.0, 86.0),
            ink_faint: Oklch::new(0.5795, 0.0, 86.0),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.7253, 0.1752, 349.8),
                type_: Oklch::new(0.7971, 0.1339, 211.5),
                function: Oklch::new(0.8493, 0.2073, 128.8),
                attribute: Oklch::new(0.709, 0.1592, 293.5),
                comment: Oklch::new(0.4855, 0.0, 86.0),
                string: Oklch::new(0.8369, 0.1644, 84.4),
                number: Oklch::new(0.7535, 0.139, 232.7),
                constant: Oklch::new(0.7192, 0.169, 13.4),
            },
            scrim: Oklch::new(0.0, 0.0, 0.0).with_alpha(0.75),
            shadow: Oklch::new(0.0, 0.0, 0.0).with_alpha(0.80),
            mode_l: 0.80,
            mode_c: 0.19,
        }
    }

    /// Every theme that ships in the binary, by the name config and the settings panel use.
    pub const BUILT_IN: [BuiltIn; 5] = [
        ("dark", Self::dark),
        ("light", Self::light),
        ("rose-pine", Self::rose_pine),
        ("rose-pine-moon", Self::rose_pine_moon),
        ("amoled", Self::amoled),
    ];

    /// Looks a built-in theme up by name. Names are slugs, as they are written in config.
    pub fn by_name(name: &str) -> Option<Self> {
        let wanted = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        Self::BUILT_IN
            .iter()
            .find(|(known, _)| *known == wanted)
            .map(|(_, build)| build())
    }

    /// The name of this theme, if it is one of the built-ins.
    pub fn name(&self) -> Option<&'static str> {
        Self::BUILT_IN
            .iter()
            .find(|(_, build)| {
                build().bg.to_hex() == self.bg.to_hex()
                    && build().syntax.keyword.to_hex() == self.syntax.keyword.to_hex()
            })
            .map(|(name, _)| *name)
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

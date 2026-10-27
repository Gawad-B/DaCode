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

impl Appearance {
    /// Both, in the order lists show them: dark first, because the default is dark.
    pub const ALL: [Appearance; 2] = [Appearance::Dark, Appearance::Light];

    /// The heading a list of themes files this appearance under.
    pub const fn title(self) -> &'static str {
        match self {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        }
    }

    pub const fn opposite(self) -> Appearance {
        match self {
            Appearance::Dark => Appearance::Light,
            Appearance::Light => Appearance::Dark,
        }
    }

    /// What a canvas of this colour is: light from half-way up OKLCH's lightness, dark below. The
    /// one rule for a theme that does not say — a theme file, an imported palette — and the rule
    /// every built-in's own declaration is tested against.
    pub fn of_background(bg: Oklch) -> Appearance {
        if bg.l < 0.5 {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    }
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

    /// The brand colour: the logo, the bell's unread dot, the activity bar's marker, the explorer's
    /// selection. Fixed where the mode accents move, because a marker that turned green on entering
    /// insert mode would read as a change of state rather than as the place it marks.
    pub primary: Oklch,

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
        Self::dacode()
    }
}

impl Theme {
    /// The default: the palette of the DaCode interface design — a cool charcoal with the chrome a
    /// shade darker than the code, and syntax colours in the family most editors' dark themes share,
    /// so nobody arriving from VSCode or Zed has to relearn what a string looks like.
    pub const fn dacode() -> Self {
        Self {
            primary: Oklch::new(0.6467, 0.1912, 256.2),
            appearance: Appearance::Dark,
            bg: Oklch::new(0.2713, 0.0086, 255.6),
            bg_alt: Oklch::new(0.2417, 0.0073, 248.1),
            bg_panel: Oklch::new(0.2332, 0.0073, 248.1),
            row_active: Oklch::new(0.3077, 0.0122, 258.4),
            rule: Oklch::new(0.3152, 0.0105, 254.0),
            rule_soft: Oklch::new(0.2840, 0.0101, 260.7),
            ink: Oklch::new(0.8878, 0.0088, 264.5),
            ink_dim: Oklch::new(0.7203, 0.0236, 263.2),
            ink_faint: Oklch::new(0.5085, 0.0209, 261.3),
            scrim: Oklch::new(0.1717, 0.0053, 248.1).with_alpha(0.62),
            shadow: Oklch::new(0.050, 0.000, 0.0).with_alpha(0.55),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.6936, 0.1635, 318.2),
                type_: Oklch::new(0.8249, 0.0970, 82.3),
                function: Oklch::new(0.7304, 0.1213, 245.3),
                attribute: Oklch::new(0.7273, 0.0949, 63.8),
                comment: Oklch::new(0.5514, 0.0190, 251.0),
                string: Oklch::new(0.7683, 0.1103, 133.0),
                number: Oklch::new(0.7273, 0.0949, 63.8),
                constant: Oklch::new(0.6709, 0.1448, 17.0),
            },
            mode_l: 0.72,
            mode_c: 0.13,
        }
    }

    pub const fn dark() -> Self {
        Self {
            primary: Oklch::new(0.6600, 0.1600, 255.0),
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
            primary: Oklch::new(0.5500, 0.1700, 255.0),
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
            "primary" | "surfaces.primary" => &mut self.primary,
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
    pub const TOKENS: [&'static str; 20] = [
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
        "primary",
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
            primary: Oklch::new(0.7300, 0.0800, 300.0),
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
            primary: Oklch::new(0.7300, 0.0800, 300.0),
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
            primary: Oklch::new(0.7000, 0.1800, 250.0),
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

    /// Quizzy, dark: an espresso room with cream ink and a teal accent — Quizzy's default palette.
    ///
    /// The surfaces, text, hairlines and accent are Quizzy's tokens exactly (`quizzy.css`). Quizzy
    /// has no syntax colours, so these are derived: one lightness band and one chroma, with hues
    /// from the palette — teal keywords (the accent), amber types (its "support"), green strings
    /// (its "progress"), orange numbers (its "attention"). The mode accents take the lightness and
    /// chroma of Quizzy's four status colours, whose hues already sit on the mode hues.
    pub const fn quizzy_dark() -> Self {
        Self {
            primary: Oklch::new(0.7038, 0.1230, 182.5),
            appearance: Appearance::Dark,
            bg: Oklch::new(0.2161, 0.0061, 56.0),
            bg_alt: Oklch::new(0.2423, 0.0062, 45.2),
            bg_panel: Oklch::new(0.2685, 0.0063, 34.3),
            row_active: Oklch::new(0.3186, 0.0059, 39.3),
            rule: Oklch::new(0.3741, 0.0087, 67.6),
            rule_soft: Oklch::new(0.3331, 0.0088, 53.2),
            ink: Oklch::new(0.9848, 0.0013, 106.4),
            ink_dim: Oklch::new(0.7161, 0.0091, 56.3),
            ink_faint: Oklch::new(0.5534, 0.0116, 58.1),
            scrim: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.6),
            shadow: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.55),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.77, 0.11, 182.0),
                type_: Oklch::new(0.84, 0.11, 72.0),
                function: Oklch::new(0.79, 0.11, 250.0),
                attribute: Oklch::new(0.71, 0.11, 300.0),
                comment: Oklch::new(0.64, 0.02, 56.0),
                string: Oklch::new(0.81, 0.11, 128.0),
                number: Oklch::new(0.79, 0.11, 42.0),
                constant: Oklch::new(0.75, 0.11, 12.0),
            },
            mode_l: 0.74,
            mode_c: 0.18,
        }
    }

    /// Quizzy, light: a cream page with espresso ink and a deep teal accent. The light side of
    /// [`Theme::quizzy_dark`], with the same derived syntax hues set dark enough for 4.5:1 on paper.
    pub const fn quizzy_light() -> Self {
        Self {
            primary: Oklch::new(0.5109, 0.0861, 186.4),
            appearance: Appearance::Light,
            bg: Oklch::new(0.9848, 0.0013, 106.4),
            bg_alt: Oklch::new(0.9924, 0.0007, 106.4),
            bg_panel: Oklch::new(1.0000, 0.0000, 89.9),
            row_active: Oklch::new(0.9464, 0.0016, 103.9),
            rule: Oklch::new(0.8687, 0.0043, 56.4),
            rule_soft: Oklch::new(0.9232, 0.0026, 48.7),
            ink: Oklch::new(0.2161, 0.0061, 56.0),
            ink_dim: Oklch::new(0.4444, 0.0096, 73.6),
            ink_faint: Oklch::new(0.7161, 0.0091, 56.3),
            scrim: Oklch::new(0.2161, 0.0061, 56.0).with_alpha(0.4),
            shadow: Oklch::new(0.2161, 0.0061, 56.0).with_alpha(0.22),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.47, 0.12, 182.0),
                type_: Oklch::new(0.50, 0.12, 72.0),
                function: Oklch::new(0.48, 0.12, 250.0),
                attribute: Oklch::new(0.53, 0.12, 300.0),
                comment: Oklch::new(0.53, 0.02, 56.0),
                string: Oklch::new(0.48, 0.12, 128.0),
                number: Oklch::new(0.51, 0.12, 42.0),
                constant: Oklch::new(0.49, 0.12, 12.0),
            },
            mode_l: 0.54,
            mode_c: 0.16,
        }
    }

    /// Charcoal coral, dark: a charcoal room, slate panels, coral for the brand and indigo beside
    /// it (`coral.css`). Keywords take the coral, functions the indigo, types the gold.
    pub const fn coral_dark() -> Self {
        Self {
            primary: Oklch::new(0.6846, 0.2042, 24.3),
            appearance: Appearance::Dark,
            bg: Oklch::new(0.1596, 0.0203, 265.6),
            bg_alt: Oklch::new(0.2196, 0.0286, 262.8),
            bg_panel: Oklch::new(0.2795, 0.0368, 260.0),
            row_active: Oklch::new(0.3277, 0.0347, 259.2),
            rule: Oklch::new(0.3717, 0.0392, 257.3),
            rule_soft: Oklch::new(0.3161, 0.0412, 261.3),
            ink: Oklch::new(0.9683, 0.0069, 247.9),
            ink_dim: Oklch::new(0.7107, 0.0351, 256.8),
            ink_faint: Oklch::new(0.6530, 0.0367, 259.4),
            scrim: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.6),
            shadow: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.55),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.77, 0.11, 22.0),
                type_: Oklch::new(0.84, 0.11, 92.0),
                function: Oklch::new(0.79, 0.11, 278.0),
                attribute: Oklch::new(0.71, 0.11, 318.0),
                comment: Oklch::new(0.64, 0.02, 255.0),
                string: Oklch::new(0.81, 0.11, 162.0),
                number: Oklch::new(0.79, 0.11, 58.0),
                constant: Oklch::new(0.75, 0.11, 205.0),
            },
            mode_l: 0.79,
            mode_c: 0.17,
        }
    }

    /// Charcoal coral, light: slate-white with near-black ink and a deeper coral.
    pub const fn coral_light() -> Self {
        Self {
            primary: Oklch::new(0.5891, 0.1960, 25.6),
            appearance: Appearance::Light,
            bg: Oklch::new(0.9683, 0.0069, 247.9),
            bg_alt: Oklch::new(0.9841, 0.0034, 247.9),
            bg_panel: Oklch::new(1.0000, 0.0000, 89.9),
            row_active: Oklch::new(0.9278, 0.0075, 248.8),
            rule: Oklch::new(0.8690, 0.0198, 252.9),
            rule_soft: Oklch::new(0.9288, 0.0126, 255.5),
            ink: Oklch::new(0.1596, 0.0203, 265.6),
            ink_dim: Oklch::new(0.4455, 0.0374, 257.3),
            ink_faint: Oklch::new(0.5544, 0.0407, 257.4),
            scrim: Oklch::new(0.1596, 0.0203, 265.6).with_alpha(0.4),
            shadow: Oklch::new(0.1596, 0.0203, 265.6).with_alpha(0.22),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.47, 0.12, 22.0),
                type_: Oklch::new(0.50, 0.12, 92.0),
                function: Oklch::new(0.48, 0.12, 278.0),
                attribute: Oklch::new(0.53, 0.12, 318.0),
                comment: Oklch::new(0.53, 0.02, 255.0),
                string: Oklch::new(0.48, 0.12, 162.0),
                number: Oklch::new(0.51, 0.12, 58.0),
                constant: Oklch::new(0.49, 0.12, 205.0),
            },
            mode_l: 0.52,
            mode_c: 0.13,
        }
    }

    /// Abyss sky, dark: an abyss-teal room with deep teal panels and sky-blue actions (`sky.css`).
    /// Keywords take the sky blue and functions the teal of its focus ring.
    pub const fn sky_dark() -> Self {
        Self {
            primary: Oklch::new(0.7535, 0.1390, 232.7),
            appearance: Appearance::Dark,
            bg: Oklch::new(0.1840, 0.0247, 219.8),
            bg_alt: Oklch::new(0.2307, 0.0336, 213.7),
            bg_panel: Oklch::new(0.2775, 0.0426, 207.6),
            row_active: Oklch::new(0.3246, 0.0404, 207.7),
            rule: Oklch::new(0.4063, 0.0559, 205.6),
            rule_soft: Oklch::new(0.3402, 0.0488, 207.6),
            ink: Oklch::new(0.9505, 0.0120, 209.8),
            ink_dim: Oklch::new(0.7705, 0.0346, 220.7),
            ink_faint: Oklch::new(0.6669, 0.0366, 220.1),
            scrim: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.6),
            shadow: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.55),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.77, 0.11, 237.0),
                type_: Oklch::new(0.84, 0.11, 75.0),
                function: Oklch::new(0.79, 0.11, 178.0),
                attribute: Oklch::new(0.71, 0.11, 292.0),
                comment: Oklch::new(0.64, 0.02, 215.0),
                string: Oklch::new(0.81, 0.11, 135.0),
                number: Oklch::new(0.79, 0.11, 45.0),
                constant: Oklch::new(0.75, 0.11, 335.0),
            },
            mode_l: 0.76,
            mode_c: 0.17,
        }
    }

    /// Abyss sky, light: snow, with the deeper sky Quizzy uses so white on it still passes AA.
    pub const fn sky_light() -> Self {
        Self {
            primary: Oklch::new(0.5000, 0.1193, 242.7),
            appearance: Appearance::Light,
            bg: Oklch::new(0.9842, 0.0034, 247.9),
            bg_alt: Oklch::new(0.9921, 0.0017, 247.9),
            bg_panel: Oklch::new(1.0000, 0.0000, 89.9),
            row_active: Oklch::new(0.9441, 0.0045, 246.5),
            rule: Oklch::new(0.8510, 0.0217, 215.3),
            rule_soft: Oklch::new(0.9205, 0.0125, 215.8),
            ink: Oklch::new(0.1840, 0.0247, 219.8),
            ink_dim: Oklch::new(0.4496, 0.0354, 220.6),
            ink_faint: Oklch::new(0.5510, 0.0359, 221.8),
            scrim: Oklch::new(0.1840, 0.0247, 219.8).with_alpha(0.4),
            shadow: Oklch::new(0.1840, 0.0247, 219.8).with_alpha(0.22),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.47, 0.12, 237.0),
                type_: Oklch::new(0.50, 0.12, 75.0),
                function: Oklch::new(0.48, 0.12, 178.0),
                attribute: Oklch::new(0.53, 0.12, 292.0),
                comment: Oklch::new(0.53, 0.02, 215.0),
                string: Oklch::new(0.48, 0.12, 135.0),
                number: Oklch::new(0.51, 0.12, 45.0),
                constant: Oklch::new(0.49, 0.12, 335.0),
            },
            mode_l: 0.55,
            mode_c: 0.17,
        }
    }

    /// Quizzy's AMOLED black: a true #000000 canvas with near-black panels, warm off-white text and
    /// teal. Not [`Theme::amoled`], which is the editor's own and stays under its name: this is the
    /// one from Quizzy, dark only by design — Quizzy gives it Quizzy light as its light side.
    pub const fn amoled_black() -> Self {
        Self {
            primary: Oklch::new(0.7038, 0.1230, 182.5),
            appearance: Appearance::Dark,
            bg: Oklch::new(0.0000, 0.0000, 0.0),
            bg_alt: Oklch::new(0.0748, 0.0000, 89.9),
            bg_panel: Oklch::new(0.1496, 0.0000, 89.9),
            row_active: Oklch::new(0.2070, 0.0001, 106.4),
            rule: Oklch::new(0.2850, 0.0000, 89.9),
            rule_soft: Oklch::new(0.2264, 0.0000, 89.9),
            ink: Oklch::new(0.9699, 0.0013, 106.4),
            ink_dim: Oklch::new(0.7161, 0.0091, 56.3),
            ink_faint: Oklch::new(0.6168, 0.0115, 67.6),
            scrim: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.75),
            shadow: Oklch::new(0.0000, 0.0000, 0.0).with_alpha(0.8),
            syntax: SyntaxPalette {
                keyword: Oklch::new(0.77, 0.12, 178.0),
                type_: Oklch::new(0.84, 0.12, 75.0),
                function: Oklch::new(0.79, 0.12, 245.0),
                attribute: Oklch::new(0.71, 0.12, 305.0),
                comment: Oklch::new(0.62, 0.02, 56.0),
                string: Oklch::new(0.81, 0.12, 130.0),
                number: Oklch::new(0.79, 0.12, 45.0),
                constant: Oklch::new(0.75, 0.12, 10.0),
            },
            mode_l: 0.76,
            mode_c: 0.17,
        }
    }

    /// Every theme that ships in the binary, by the name config and the settings panel use.
    pub const BUILT_IN: [BuiltIn; 13] = [
        ("dacode", Self::dacode),
        ("dark", Self::dark),
        ("light", Self::light),
        ("rose-pine", Self::rose_pine),
        ("rose-pine-moon", Self::rose_pine_moon),
        ("amoled", Self::amoled),
        ("quizzy-dark", Self::quizzy_dark),
        ("quizzy-light", Self::quizzy_light),
        ("coral-dark", Self::coral_dark),
        ("coral-light", Self::coral_light),
        ("sky-dark", Self::sky_dark),
        ("sky-light", Self::sky_light),
        ("amoled-black", Self::amoled_black),
    ];

    /// The built-ins of one appearance, in the order [`Theme::BUILT_IN`] lists them.
    pub fn built_ins(appearance: Appearance) -> impl Iterator<Item = &'static str> {
        Self::BUILT_IN
            .iter()
            .filter(move |(_, build)| build().appearance == appearance)
            .map(|(name, _)| *name)
    }

    /// Every built-in, the dark ones first and then the light: the order every list of themes shows
    /// them in — the settings tab's dropdown under its two headings, the wizard's cards, and the
    /// arrows on the theme row — so a theme is in the same place wherever it is chosen from.
    pub fn grouped() -> Vec<&'static str> {
        Appearance::ALL
            .into_iter()
            .flat_map(Self::built_ins)
            .collect()
    }

    /// The same theme in the other appearance, for `toggle_theme`.
    ///
    /// A `-dark` theme with a `-light` twin swaps to it and back, so toggling Charcoal coral stays
    /// Charcoal coral. AMOLED black has no light side; Quizzy gives it Quizzy light, and so does this.
    /// Anything else goes to the plain [`Theme::dark`] or [`Theme::light`], which is what the toggle
    /// always did.
    pub fn counterpart(&self) -> Theme {
        let twin = self.name().and_then(|name| match name {
            "amoled-black" => Some("quizzy-light".to_string()),
            _ => name
                .strip_suffix("-dark")
                .map(|stem| format!("{stem}-light"))
                .or_else(|| {
                    name.strip_suffix("-light")
                        .map(|stem| format!("{stem}-dark"))
                }),
        });
        twin.and_then(|name| Self::by_name(&name))
            .unwrap_or_else(|| Self::for_appearance(self.appearance.opposite()))
    }

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

    /// The logo's amber — the block in the mark and "Da" in the wordmark.
    ///
    /// Taken from the logo sheet, which gives it two values: the bright one on dark surfaces and a
    /// deeper one on light, where the bright one would read as yellow on white. Fixed per appearance
    /// rather than per theme, because it is the brand's colour, not the theme's.
    pub const fn brand(&self) -> Oklch {
        match self.appearance {
            Appearance::Dark => Oklch::new(0.8134, 0.1404, 80.2),
            Appearance::Light => Oklch::new(0.6212, 0.1290, 72.8),
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

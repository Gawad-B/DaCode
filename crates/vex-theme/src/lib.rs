//! `vex-theme` — the editor's visual language: color, mode accents, type roles, spacing.
//!
//! Derived from the design handoff, but encoded as *relationships* rather than as a copy of its
//! markup: the mock is a reference for look and behavior, and the geometry it specifies is
//! guidance for the render layer, not a stylesheet to reproduce.
//!
//! The one non-negotiable idea is **mode is the environment**. A single [`Mode`] value drives
//! every accent in the window through [`Theme::accent`], so a mode change moves the gutter
//! wash, cursor line, cursor shape, tab underline, statusline and panel highlights together.
//! Nothing downstream may hardcode a mode hue.

pub mod color;
pub mod mode;
pub mod theme;
pub mod typography;

pub use color::Oklch;
pub use mode::{CursorShape, Mode};
pub use theme::{Appearance, BuiltIn, SyntaxPalette, Theme};
pub use typography::{Family, FontSpec, Role, Style, Typography, frame, space};

/// Frame budget in milliseconds. Not decoration: the statusline shows measured frame time
/// against this number, because a performance target you cannot see is a target you will lose.
pub const FRAME_BUDGET_MS: f32 = 16.6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_accents_differ_only_in_hue() {
        // The identity claim: one instrument changing state, not four themes. If a future edit
        // gives one mode its own lightness or chroma, this fails.
        let theme = Theme::dark();
        let accents: Vec<_> = Mode::ALL.iter().map(|&m| theme.accent(m)).collect();

        for accent in &accents {
            assert_eq!(accent.l, accents[0].l, "all modes share one lightness");
            assert_eq!(accent.c, accents[0].c, "all modes share one chroma");
        }

        let mut hues: Vec<_> = accents.iter().map(|a| a.h).collect();
        hues.sort_by(f32::total_cmp);
        hues.dedup();
        assert_eq!(hues.len(), 4, "all four hues must be distinct");
    }

    #[test]
    fn light_theme_darkens_accents() {
        // Accents must stay legible on paper: same hue, lower lightness.
        for mode in Mode::ALL {
            let dark = Theme::dark().accent(mode);
            let light = Theme::light().accent(mode);
            assert_eq!(dark.h, light.h);
            assert!(light.l < dark.l, "{mode:?} must darken in the light theme");
        }
    }

    #[test]
    fn oklch_converts_to_plausible_srgb() {
        // Anchors for the conversion math, which is otherwise easy to get subtly wrong.
        assert_eq!(Oklch::new(1.0, 0.0, 0.0).to_hex(), "#ffffff");
        assert_eq!(Oklch::new(0.0, 0.0, 0.0).to_hex(), "#000000");

        let bg = Theme::dark().bg.to_srgb8();
        assert!(
            bg[0] < 60 && bg[1] < 60 && bg[2] < 60,
            "dark canvas must be dark: {bg:?}"
        );

        let ink = Theme::dark().ink.to_srgb8();
        assert!(ink[0] > 200, "primary text must be bright: {ink:?}");
    }

    #[test]
    fn tints_resolve_to_opaque_colors_near_the_canvas() {
        // Tints are specified at 6% alpha; resolved they must be visibly distinct from the
        // canvas but nowhere near the accent itself.
        let theme = Theme::dark();
        let bg = theme.bg.to_linear();
        let tint = theme.line_tint(Mode::Normal);

        assert_eq!(tint[3], 1.0, "must be composited to opaque");
        let delta: f32 = (0..3).map(|i| (tint[i] - bg[i]).abs()).sum();
        assert!(
            delta > 0.0005,
            "tint must be distinguishable from the canvas"
        );
        assert!(delta < 0.15, "tint must stay subtle");
    }

    #[test]
    fn gutter_and_code_rows_are_identical() {
        // Divergent row heights would unhook line numbers from their lines.
        let typo = Typography::default();
        assert_eq!(
            typo.spec(Role::LineNumber).line_height,
            typo.spec(Role::Code).line_height
        );
        assert_eq!(typo.row_height(), (13.5 * 1.9_f32).round());
    }

    #[test]
    fn comments_are_set_as_code() {
        // This was the design's most distinctive typographic decision and it is deliberately gone:
        // comments were italic serif a size larger than the code around them. It is a real idea —
        // prose among instructions — and it is the thing people noticed first and liked least, and
        // a proportional face cannot hold a monospaced column, so a commented-out line sat visibly
        // out of step with the line it was commenting out. What distinguishes a comment now is the
        // colour the theme gives it, which is what was doing the work anyway.
        let typo = Typography::default();
        let comment = typo.spec(Role::Comment);
        let code = typo.spec(Role::Code);

        assert_eq!(comment.family, Family::Mono, "the same face as the code");
        assert_eq!(comment.style, Style::Roman, "upright");
        assert_eq!(comment.size_px, code.size_px, "and the same size");
        assert_eq!(
            comment.line_height, code.line_height,
            "must share the row grid"
        );
    }

    /// The wordmark keeps the serif italic that every complaint about the code face spared.
    #[test]
    fn the_display_face_is_still_the_serif() {
        let typo = Typography::default();
        let display = typo.spec(Role::Display);
        assert_eq!(display.family, Family::Serif);
        assert_eq!(display.style, Style::Italic);
    }

    #[test]
    fn cursor_shape_distinguishes_modes_without_color() {
        // Shape carries the mode before color does.
        assert!(matches!(
            Mode::Insert.cursor(),
            CursorShape::Bar { blink: true, .. }
        ));
        assert!(matches!(
            Mode::Normal.cursor(),
            CursorShape::Block {
                underline: true,
                ..
            }
        ));
        assert!(matches!(Mode::Replace.cursor(), CursorShape::Solid { .. }));

        // A resting normal-mode cursor must not blink: blinking reads as "waiting for text"
        // when it is actually waiting for a command.
        assert!(
            !matches!(Mode::Normal.cursor(), CursorShape::Bar { blink: true, .. }),
            "normal mode must not use a blinking bar"
        );
    }

    #[test]
    fn code_size_is_clamped_to_the_supported_range() {
        assert_eq!(
            Typography::with_code_size(4.0).code_size,
            Typography::MIN_CODE_SIZE
        );
        assert_eq!(
            Typography::with_code_size(99.0).code_size,
            Typography::MAX_CODE_SIZE
        );
    }

    #[test]
    fn syntax_colors_are_one_family_and_all_distinct() {
        // Two roles sharing a color makes them indistinguishable; a role outside the family's
        // chroma range makes the code area look like two themes fighting.
        let theme = Theme::dark();
        let syntax = theme.syntax;
        let colors = [
            syntax.keyword,
            syntax.type_,
            syntax.function,
            syntax.attribute,
            syntax.string,
            syntax.number,
            syntax.constant,
        ];

        let mut hex: Vec<String> = colors.iter().map(|c| c.to_hex()).collect();
        hex.sort();
        hex.dedup();
        assert_eq!(
            hex.len(),
            colors.len(),
            "syntax roles must be distinguishable"
        );

        for color in colors {
            assert!(
                (0.08..=0.14).contains(&color.c),
                "syntax chroma out of family range: {color:?}"
            );
            assert!(
                color.l > 0.6,
                "dark-theme syntax must stay legible: {color:?}"
            );
        }

        // The comment is the deliberate exception: near-neutral, because it is set as prose.
        assert!(
            syntax.comment.c < 0.05,
            "comments are near-neutral by design"
        );
    }

    #[test]
    fn srgb_round_trips_through_oklch() {
        // Theme import depends on this: a hex color that does not survive the round trip would be
        // silently shifted, and every imported theme would look subtly wrong.
        for hex in [
            "#000000", "#ffffff", "#1e1e1e", "#569cd6", "#ce9178", "#4ec9b0",
        ] {
            let color = Oklch::from_hex(hex).expect("parses");
            assert_eq!(color.to_hex(), hex, "{hex} must round-trip");
        }
    }

    #[test]
    fn hex_shorthand_and_alpha_are_accepted() {
        assert_eq!(Oklch::from_hex("#fff").unwrap().to_hex(), "#ffffff");
        let translucent = Oklch::from_hex("#ffffff80").unwrap();
        assert!((translucent.alpha - 0.502).abs() < 0.01);
        assert!(Oklch::from_hex("not-a-color").is_none());
    }

    #[test]
    fn everything_is_square() {
        assert_eq!(frame::RADIUS, 0.0);
    }

    #[test]
    fn every_token_name_sets_a_distinct_field() {
        // A token that silently does nothing looks to the user exactly like a broken theme, so the
        // table is checked rather than trusted: each name must accept a color, and the 19 names must
        // reach 19 different fields.
        let marker = Oklch::new(0.5, 0.2, 123.0);
        let mut seen = std::collections::HashSet::new();

        for name in Theme::TOKENS {
            let mut theme = Theme::dark();
            assert!(theme.set_token(name, marker), "`{name}` was rejected");

            let changed: Vec<String> = Theme::TOKENS
                .iter()
                .filter(|other| {
                    let mut probe = Theme::dark();
                    probe.set_token(other, marker);
                    // Compare the whole theme: if setting `name` moved the same field as `other`,
                    // the two probes are identical.
                    format!("{:?}", probe) == format!("{:?}", theme)
                })
                .map(|s| s.to_string())
                .collect();

            assert_eq!(changed, vec![name.to_string()], "`{name}` is not distinct");
            assert!(seen.insert(name), "`{name}` appears twice");
        }

        assert!(!Theme::dark().set_token("bakcground", marker));
    }

    #[test]
    fn syntax_tokens_are_reachable_bare_and_qualified() {
        let color = Oklch::new(0.7, 0.16, 318.0);
        let mut bare = Theme::dark();
        let mut qualified = Theme::dark();

        assert!(bare.set_token("keyword", color));
        assert!(qualified.set_token("syntax.keyword", color));
        assert_eq!(
            bare.syntax.keyword.to_hex(),
            qualified.syntax.keyword.to_hex()
        );

        // `theme.` prefixed names come from the TOML reader's flattened keys.
        let mut prefixed = Theme::dark();
        assert!(prefixed.set_token("theme.surfaces.bg", color));
        assert_eq!(prefixed.bg.to_hex(), color.to_hex());
    }

    #[test]
    fn colors_parse_from_both_notations() {
        let from_oklch = Oklch::parse("oklch(0.694 0.164 318.2)").expect("oklch parses");
        assert!((from_oklch.l - 0.694).abs() < 0.001);
        assert!((from_oklch.h - 318.2).abs() < 0.1);

        let from_hex = Oklch::parse("#c678dd").expect("hex parses");
        assert_eq!(from_hex.to_hex(), "#c678dd");

        // Round-tripping hex through OKLCH must not shift the color, or an imported theme drifts
        // every time it is saved.
        assert_eq!(Oklch::parse("#ffffff").unwrap().to_hex(), "#ffffff");
        assert_eq!(Oklch::parse("#000000").unwrap().to_hex(), "#000000");

        assert!(Oklch::parse("rgb(1,2,3)").is_none());
        assert!(
            Oklch::parse("oklch(0.5 0.1)").is_none(),
            "three parts required"
        );
        assert!(Oklch::parse("oklch(0.5 0.1 20 30)").is_none());
    }

    #[test]
    fn every_built_in_theme_is_reachable_by_name_and_names_itself() {
        for (name, build) in Theme::BUILT_IN {
            let theme = build();
            assert!(
                Theme::by_name(name).is_some(),
                "`{name}` is listed but cannot be looked up"
            );
            assert_eq!(
                theme.name(),
                Some(name),
                "a theme must recognise itself, or the settings panel cannot show which is active"
            );
        }

        // Names are slugs, and people type them with spaces and underscores.
        assert!(Theme::by_name("Rose Pine").is_some());
        assert!(Theme::by_name("rose_pine_moon").is_some());
        assert!(Theme::by_name("nonsense").is_none());
    }

    #[test]
    fn every_built_in_theme_is_legible() {
        // The one property a theme cannot get wrong: text has to be distinguishable from the surface
        // it sits on, and every syntax colour from the background. Checked in OKLCH lightness, which
        // is perceptual — the whole reason the palette is stored this way.
        for (name, build) in Theme::BUILT_IN {
            let theme = build();
            let gap = (theme.ink.l - theme.bg.l).abs();
            assert!(
                gap > 0.45,
                "{name}: ink and background are only {gap:.2} apart in lightness"
            );

            for (role, color) in [
                ("keyword", theme.syntax.keyword),
                ("type", theme.syntax.type_),
                ("function", theme.syntax.function),
                ("comment", theme.syntax.comment),
                ("string", theme.syntax.string),
                ("number", theme.syntax.number),
                ("constant", theme.syntax.constant),
            ] {
                let gap = (color.l - theme.bg.l).abs();
                assert!(
                    gap > 0.20,
                    "{name}: {role} is only {gap:.2} from the background in lightness"
                );
            }

            // And the panels have to be distinguishable from the canvas, or the chrome disappears.
            assert!(
                (theme.bg_panel.l - theme.bg.l).abs() > 0.01,
                "{name}: the panel surface is indistinguishable from the canvas"
            );
        }
    }

    #[test]
    fn amoled_is_actually_black() {
        // The point of the theme: those pixels are off on an OLED panel. A near-black would not be.
        assert_eq!(Theme::amoled().bg.to_hex(), "#000000");
    }

    #[test]
    fn mode_accents_stay_distinct_in_every_theme() {
        for (name, build) in Theme::BUILT_IN {
            let theme = build();
            let mut colors: Vec<String> = Mode::ALL
                .iter()
                .map(|mode| theme.accent(*mode).to_hex())
                .collect();
            colors.sort();
            colors.dedup();
            assert_eq!(colors.len(), 4, "{name}: modes must stay visually distinct");
        }
    }
}

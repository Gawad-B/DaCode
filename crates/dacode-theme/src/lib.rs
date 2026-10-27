//! `dacode-theme` — the editor's visual language: color, mode accents, type roles, spacing.
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
pub use typography::{FACES, FontSpec, Role, Style, Typography, frame, space};

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

        assert_eq!(comment.style, Style::Roman, "upright");
        assert_eq!(comment.size_px, code.size_px, "and the same size");
        assert_eq!(
            comment.line_height, code.line_height,
            "must share the row grid"
        );
    }

    /// One face, so nothing asks for an italic it does not have: a request for one would be met by
    /// whatever italic the system happened to have, in a different face from everything around it.
    #[test]
    fn every_role_is_upright_in_the_one_face() {
        let typo = Typography::default();
        for role in [
            Role::Display,
            Role::Prose,
            Role::Code,
            Role::Comment,
            Role::LineNumber,
            Role::Value,
            Role::Label,
            Role::Ui,
            Role::Brand,
        ] {
            assert_eq!(typo.spec(role).style, Style::Roman, "{role:?}");
        }
        assert_eq!(crate::FACES.first(), Some(&"Fira Code"));
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
        for (name, _, build) in Theme::BUILT_IN {
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
        for (name, _, build) in Theme::BUILT_IN {
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

    /// The names the seven themes taken from Quizzy go by. A list, because a test that only iterated
    /// `BUILT_IN` would pass with any of them missing.
    const QUIZZY: [&str; 7] = [
        "quizzy-dark",
        "quizzy-light",
        "coral-dark",
        "coral-light",
        "sky-dark",
        "sky-light",
        "amoled-black",
    ];

    #[test]
    fn every_built_in_says_whether_it_is_dark_or_light_and_means_it() {
        // The settings tab lists themes under "Dark" and "Light", and a theme filed under the wrong
        // one is a white page found under "Dark". So the declared appearance is checked against what
        // the theme looks like: its canvas, the same test a theme file is classified by.
        for (name, _, build) in Theme::BUILT_IN {
            let theme = build();
            assert_eq!(
                theme.appearance,
                Appearance::of_background(theme.bg),
                "{name} is filed under the wrong appearance"
            );
        }
        for appearance in Appearance::ALL {
            assert!(
                Theme::built_ins(appearance).next().is_some(),
                "{appearance:?} has no themes, so its heading would head nothing"
            );
        }
        assert_eq!(Appearance::Dark.title(), "Dark");
        assert_eq!(Appearance::Light.title(), "Light");
        assert_eq!(
            Appearance::of_background(Oklch::new(0.98, 0.0, 0.0)),
            Appearance::Light
        );
        assert_eq!(
            Appearance::of_background(Oklch::new(0.0, 0.0, 0.0)),
            Appearance::Dark
        );
    }

    #[test]
    fn the_grouped_order_lists_every_built_in_once_dark_first() {
        // Every list of themes — the dropdown, the wizard, the arrows on the theme row — walks this
        // order, so a theme missing from it is a theme nobody can pick.
        let grouped = Theme::grouped();
        assert_eq!(grouped.len(), Theme::BUILT_IN.len());
        for (name, _, _) in Theme::BUILT_IN {
            assert_eq!(
                grouped.iter().filter(|listed| **listed == name).count(),
                1,
                "{name}"
            );
        }
        let first_light = grouped
            .iter()
            .position(|name| Theme::by_name(name).unwrap().appearance == Appearance::Light)
            .expect("there are light themes");
        assert!(
            grouped[first_light..]
                .iter()
                .all(|name| Theme::by_name(name).unwrap().appearance == Appearance::Light),
            "every light theme comes after every dark one: {grouped:?}"
        );
    }

    #[test]
    fn the_quizzy_themes_are_built_in() {
        for name in QUIZZY {
            assert!(Theme::by_name(name).is_some(), "{name} is not built in");
        }
        // The old one stays: `theme = "amoled"` is in people's init.lua.
        assert!(Theme::by_name("amoled").is_some());
        assert_ne!(
            Theme::by_name("amoled").unwrap(),
            Theme::by_name("amoled-black").unwrap()
        );
    }

    #[test]
    fn the_quizzy_themes_keep_quizzys_own_colours() {
        // The surfaces, the text and the accent are Quizzy's tokens exactly, so the editor and the
        // app it was asked to match are the same colours rather than near them.
        // bg, bg_panel, ink, ink_dim, ink_faint, rule, rule_soft, primary — every token taken from
        // Quizzy, so an edit to any one of them is caught, not only to the four most visible.
        let expect = [
            (
                "quizzy-dark",
                [
                    "#1c1917", "#292524", "#fafaf9", "#a8a29e", "#78716c", "#44403c", "#3a3532",
                    "#14b8a6",
                ],
            ),
            (
                "quizzy-light",
                [
                    "#fafaf9", "#ffffff", "#1c1917", "#57534e", "#a8a29e", "#d6d3d1", "#e7e5e4",
                    "#0f766e",
                ],
            ),
            (
                "coral-dark",
                [
                    "#090d16", "#1e293b", "#f1f5f9", "#94a3b8", "#8391a7", "#334155", "#263247",
                    "#ff5757",
                ],
            ),
            (
                "coral-light",
                [
                    "#f1f5f9", "#ffffff", "#090d16", "#475569", "#64748b", "#cbd5e1", "#e2e8f0",
                    "#d93a3a",
                ],
            ),
            (
                "sky-dark",
                [
                    "#05151a", "#072e33", "#e6f1f3", "#9dbac4", "#7c9aa4", "#1d5258", "#123f45",
                    "#38bdf8",
                ],
            ),
            (
                "sky-light",
                [
                    "#f8fafc", "#ffffff", "#05151a", "#3f5a63", "#5b7781", "#bfd2d7", "#dce7ea",
                    "#0369a1",
                ],
            ),
            (
                "amoled-black",
                [
                    "#000000", "#0b0b0b", "#f5f5f4", "#a8a29e", "#8a847e", "#2a2a2a", "#1c1c1c",
                    "#14b8a6",
                ],
            ),
        ];
        for (name, hex) in expect {
            let theme = Theme::by_name(name).unwrap();
            let got = [
                theme.bg,
                theme.bg_panel,
                theme.ink,
                theme.ink_dim,
                theme.ink_faint,
                theme.rule,
                theme.rule_soft,
                theme.primary,
            ]
            .map(|color| color.to_hex());
            let tokens = [
                "bg",
                "bg_panel",
                "ink",
                "ink_dim",
                "ink_faint",
                "rule",
                "rule_soft",
                "primary",
            ];
            for at in 0..tokens.len() {
                assert_eq!(got[at], hex[at], "{name} {}", tokens[at]);
            }
        }
    }

    /// WCAG's contrast ratio, from the relative luminance of two opaque colours.
    fn contrast(a: Oklch, b: Oklch) -> f32 {
        let luminance = |color: Oklch| {
            let [r, g, b, _] = color.to_linear();
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn the_quizzy_themes_syntax_is_one_family_all_distinct_and_readable() {
        // Quizzy has no syntax colours, so each theme's were derived from its palette: one lightness
        // band and one chroma per appearance, hues taken from its accent and status colours. The
        // same rules `syntax_colors_are_one_family_and_all_distinct` holds the default to, plus the
        // one lightness alone cannot promise: 4.5:1 against the canvas, comments included.
        for name in QUIZZY {
            let theme = Theme::by_name(name).unwrap();
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
            assert_eq!(hex.len(), colors.len(), "{name}: roles must differ");

            let dark = theme.appearance == Appearance::Dark;
            for color in colors {
                assert!(
                    (0.08..=0.14).contains(&color.c),
                    "{name}: chroma out of family range: {color:?}"
                );
                assert!(
                    if dark { color.l > 0.6 } else { color.l < 0.6 },
                    "{name}: lightness on the wrong side for its appearance: {color:?}"
                );
            }
            assert!(syntax.comment.c < 0.05, "{name}: comments are near-neutral");
            for color in colors.into_iter().chain([syntax.comment, theme.ink]) {
                let ratio = contrast(color, theme.bg);
                assert!(
                    ratio >= 4.5,
                    "{name}: {color:?} is only {ratio:.1}:1 on its canvas"
                );
            }
        }
    }

    #[test]
    fn a_light_quizzy_theme_darkens_the_accents_of_its_dark_one() {
        for (dark, light) in [
            ("quizzy-dark", "quizzy-light"),
            ("coral-dark", "coral-light"),
            ("sky-dark", "sky-light"),
        ] {
            let (dark, light) = (
                Theme::by_name(dark).unwrap(),
                Theme::by_name(light).unwrap(),
            );
            for mode in Mode::ALL {
                assert!(light.accent(mode).l < dark.accent(mode).l, "{mode:?}");
            }
        }
    }

    #[test]
    fn toggling_a_theme_lands_on_its_own_other_half() {
        // `toggle_theme` used to jump to `light` or `dark` from anything, so toggling a coral theme
        // threw coral away. A pair swaps within itself; AMOLED black has no light side, and Quizzy
        // gives it Quizzy light; anything else goes to the plain theme of the other appearance.
        let pairs = [
            ("quizzy-dark", "quizzy-light"),
            ("quizzy-light", "quizzy-dark"),
            ("coral-dark", "coral-light"),
            ("coral-light", "coral-dark"),
            ("sky-dark", "sky-light"),
            ("sky-light", "sky-dark"),
            ("amoled-black", "quizzy-light"),
            ("dark", "light"),
            ("light", "dark"),
            ("rose-pine", "light"),
        ];
        for (from, to) in pairs {
            let flipped = Theme::by_name(from).unwrap().counterpart();
            assert_eq!(flipped.name(), Some(to), "{from}");
        }
        for (name, _, build) in Theme::BUILT_IN {
            assert_ne!(
                build().counterpart().appearance,
                build().appearance,
                "{name}: a toggle must change the appearance"
            );
        }
    }

    #[test]
    fn every_built_in_has_a_display_name_and_a_pair_shares_one() {
        // The dropdown files themes under "Dark" and "Light", so a name that repeats the heading
        // ("quizzy-dark" under Dark) is noise: the two halves of a pair go by one name, and the heading
        // or the card's own colours say which half it is. Config keeps the slugs.
        for (name, title, build) in Theme::BUILT_IN {
            assert!(!title.is_empty(), "{name}");
            assert_eq!(Theme::display_name_of(name), Some(title));
            assert_eq!(build().display_name(), Some(title), "{name}");
            assert!(
                !title.to_lowercase().ends_with(" dark")
                    && !title.to_lowercase().ends_with(" light"),
                "{name}: `{title}` repeats its heading"
            );
        }
        for (pair, title) in [
            (["quizzy-dark", "quizzy-light"], "Quizzy"),
            (["coral-dark", "coral-light"], "Charcoal coral"),
            (["sky-dark", "sky-light"], "Abyss sky"),
        ] {
            for name in pair {
                assert_eq!(Theme::display_name_of(name), Some(title), "{name}");
            }
        }
        assert_eq!(Theme::display_name_of("amoled-black"), Some("AMOLED black"));
        assert_eq!(
            Theme::display_name_of("rose-pine-moon"),
            Some("Rosé Pine Moon")
        );
        assert_eq!(Theme::display_name_of("dacode"), Some("DaCode"));
        // Within one appearance no two share a name, or the list would show the same row twice.
        for appearance in Appearance::ALL {
            let mut titles: Vec<&str> = Theme::built_ins(appearance)
                .map(|name| Theme::display_name_of(name).unwrap())
                .collect();
            let count = titles.len();
            titles.sort_unstable();
            titles.dedup();
            assert_eq!(titles.len(), count, "{appearance:?}: {titles:?}");
        }
        // A display name is not a config name.
        assert!(Theme::by_name("Charcoal coral").is_none());
    }

    #[test]
    fn amoled_is_actually_black() {
        // The point of the theme: those pixels are off on an OLED panel. A near-black would not be.
        assert_eq!(Theme::amoled().bg.to_hex(), "#000000");
    }

    #[test]
    fn mode_accents_stay_distinct_in_every_theme() {
        for (name, _, build) in Theme::BUILT_IN {
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

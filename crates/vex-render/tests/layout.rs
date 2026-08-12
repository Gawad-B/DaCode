//! Frame-building tests.
//!
//! Building a frame involves no GPU, which is the reason the UI layer is split from the
//! renderer: layout regressions get caught in a normal `cargo test` run rather than by eye.

use vex_render::{Align, Frame, Layer, Rect, TextRun};
use vex_theme::{Mode, Role, Theme, Typography};

fn spec(role: Role) -> vex_theme::FontSpec {
    Typography::default().spec(role)
}

#[test]
fn splits_partition_exactly() {
    // Off-by-one in a split shows up as a hairline gap or a 1px overlap between panels, which
    // reads as sloppiness even when nobody can name what is wrong.
    let bounds = Rect::new(0.0, 0.0, 1280.0, 800.0);

    let (top, rest) = bounds.split_top(72.0);
    assert_eq!(top.height, 72.0);
    assert_eq!(rest.y, 72.0);
    assert_eq!(top.height + rest.height, bounds.height);

    let (right, left) = bounds.split_right(292.0);
    assert_eq!(right.width, 292.0);
    assert_eq!(right.x, 1280.0 - 292.0);
    assert_eq!(left.width + right.width, bounds.width);
    assert_eq!(left.x, bounds.x);
}

#[test]
fn splits_clamp_instead_of_going_negative() {
    // A window narrower than the sidebar must not produce negative geometry.
    let tiny = Rect::new(0.0, 0.0, 50.0, 40.0);
    let (panel, rest) = tiny.split_right(292.0);
    assert_eq!(panel.width, 50.0);
    assert_eq!(rest.width, 0.0);
    assert!(!rest.is_visible());
}

#[test]
fn zero_area_quads_are_dropped() {
    // Culling at the source keeps degenerate rects out of the instance buffer entirely.
    let mut frame = Frame::default();
    let theme = Theme::dark();

    frame.fill(Rect::new(0.0, 0.0, 0.0, 10.0), theme.ink);
    frame.fill(Rect::new(0.0, 0.0, 10.0, 0.0), theme.ink);
    assert!(frame.quads.is_empty());

    frame.fill(Rect::new(0.0, 0.0, 10.0, 10.0), theme.ink);
    assert_eq!(frame.quads.len(), 1);
}

#[test]
fn labels_are_uppercased_centrally() {
    // Enforced in `Frame::text` so no call site can ship a lowercase label by accident.
    let mut frame = Frame::default();
    frame.text(TextRun {
        text: "normal mode".into(),
        spec: spec(Role::Label),
        color: Theme::dark().ink_faint,
        rect: Rect::new(0.0, 0.0, 200.0, 20.0),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
    });

    assert_eq!(frame.texts[0].text, "NORMAL MODE");
}

#[test]
fn code_runs_are_not_uppercased() {
    let mut frame = Frame::default();
    frame.text(TextRun {
        text: "let mut offset = CharOffset(0);".into(),
        spec: spec(Role::Code),
        color: Theme::dark().ink,
        rect: Rect::new(0.0, 0.0, 400.0, 20.0),
        align: Align::Left,
        center_vertically: false,
        wrap: false,
        layer: Layer::default(),
    });

    assert_eq!(frame.texts[0].text, "let mut offset = CharOffset(0);");
}

#[test]
fn empty_text_is_dropped() {
    let mut frame = Frame::default();
    frame.text(TextRun {
        text: String::new(),
        spec: spec(Role::Code),
        color: Theme::dark().ink,
        rect: Rect::new(0.0, 0.0, 100.0, 20.0),
        align: Align::Left,
        center_vertically: false,
        wrap: false,
        layer: Layer::default(),
    });
    assert!(frame.texts.is_empty());
}

#[test]
fn gradient_covers_its_rect_without_gaps() {
    // The gutter wash is strips, not a real gradient. Strips must tile the rect completely, or
    // the canvas shows through as vertical banding.
    let mut frame = Frame::default();
    let theme = Theme::dark();
    let (from, to) = theme.gutter_tint(Mode::Normal);
    let rect = Rect::new(10.0, 20.0, 78.0, 400.0);

    frame.fill_gradient_x(rect, from, to, 24);
    assert_eq!(frame.quads.len(), 24);

    let first = &frame.quads[0];
    let last = &frame.quads[23];
    assert_eq!(first.rect.x, rect.x);
    assert!(
        last.rect.right() >= rect.right(),
        "strips must reach the right edge: {} < {}",
        last.rect.right(),
        rect.right()
    );

    // Ends interpolate toward the endpoint colors without overshooting them.
    for channel in 0..3 {
        let span = (to[channel] - from[channel]).abs();
        assert!((first.color[channel] - from[channel]).abs() <= span);
        assert!((last.color[channel] - to[channel]).abs() <= span);
    }
}

#[test]
fn rules_land_on_whole_pixels() {
    // A hairline on a fractional coordinate is resampled into a grey smear. The design uses 1px
    // rules structurally, so this has to be exact.
    let mut frame = Frame::default();
    let theme = Theme::dark();

    frame.rule_bottom(Rect::new(0.0, 0.5, 100.0, 33.7), theme.rule, 1.0);
    frame.rule_top(Rect::new(0.0, 10.3, 100.0, 20.0), theme.rule, 1.0);
    frame.rule_left(Rect::new(4.6, 0.0, 100.0, 20.0), theme.rule, 1.0);

    assert_eq!(frame.quads[0].rect.y.fract(), 0.0);
    assert_eq!(frame.quads[1].rect.y.fract(), 0.0);
    assert_eq!(frame.quads[2].rect.x.fract(), 0.0);
}

#[test]
fn mode_change_repaints_every_accent() {
    // The identity claim, tested at the frame level: switching modes must change the *set of
    // colors on screen*, not just one label. If a future refactor hardcodes a hue somewhere,
    // the count of mode-colored quads stops moving with the mode.
    let theme = Theme::dark();

    let accent_counts: Vec<usize> = Mode::ALL
        .iter()
        .map(|&mode| {
            let accent = theme.accent(mode).to_linear();
            let mut frame = Frame::default();

            // A miniature of the chrome's mode-derived elements.
            frame.fill(Rect::new(0.0, 0.0, 34.0, 3.0), theme.accent(mode));
            frame.fill(Rect::new(0.0, 10.0, 100.0, 2.0), theme.accent(mode));
            frame.fill_linear(Rect::new(0.0, 20.0, 100.0, 26.0), theme.line_tint(mode));
            let (from, to) = theme.gutter_tint(mode);
            frame.fill_gradient_x(Rect::new(0.0, 50.0, 78.0, 26.0), from, to, 4);

            frame
                .quads
                .iter()
                .filter(|quad| quad.color == accent)
                .count()
        })
        .collect();

    // Each mode paints the same number of elements in its own accent...
    assert!(accent_counts.iter().all(|&count| count == accent_counts[0]));
    assert!(accent_counts[0] >= 2);

    // ...and no two modes produce the same accent color.
    let mut colors: Vec<[u8; 4]> = Mode::ALL
        .iter()
        .map(|&mode| theme.accent(mode).to_srgb8())
        .collect();
    colors.sort();
    colors.dedup();
    assert_eq!(colors.len(), 4, "modes must be visually distinct");
}

/// An overlay must occlude what is beneath it.
///
/// The renderer draws a whole layer at a time — that layer's rectangles, then its glyphs — so a
/// panel on a higher layer covers text on a lower one. Within a *single* layer it cannot, because
/// all the rectangles go down before any of the glyphs. This test pins the property the popup
/// depends on: everything drawn after `set_layer` carries that layer, and the layers stay ordered.
#[test]
fn a_panel_layer_carries_everything_drawn_after_it() {
    let theme = Theme::dark();
    let mut frame = Frame::default();

    // Base: some code.
    frame.fill(Rect::new(0.0, 0.0, 200.0, 100.0), theme.bg);
    frame.text(run("fn main() {", Rect::new(0.0, 0.0, 200.0, 20.0)));

    // A completion popup over it.
    frame.set_layer(Layer::Panel);
    frame.fill(Rect::new(20.0, 10.0, 100.0, 40.0), theme.bg_panel);
    frame.text(run("char_to_point", Rect::new(20.0, 10.0, 100.0, 20.0)));

    frame.set_layer(Layer::Base);
    frame.text(run("statusline", Rect::new(0.0, 80.0, 200.0, 20.0)));

    assert_eq!(frame.quads_on(Layer::Panel).count(), 1);
    assert_eq!(
        frame
            .texts_on(Layer::Panel)
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>(),
        vec!["char_to_point"],
        "only the popup's own text is on its layer"
    );
    assert_eq!(
        frame.texts_on(Layer::Base).count(),
        2,
        "drawing returns to the base layer when asked"
    );
    assert_eq!(frame.quads_on(Layer::Modal).count(), 0);

    // Paint order: a panel is drawn after the base, a modal after the panel.
    assert!(Layer::Base < Layer::Panel && Layer::Panel < Layer::Modal);
    assert_eq!(Layer::default(), Layer::Base);
}

fn run(text: &str, rect: Rect) -> TextRun {
    TextRun {
        text: text.to_string(),
        spec: Typography::default().spec(Role::Code),
        color: Theme::dark().ink,
        rect,
        align: Align::Left,
        center_vertically: false,
        wrap: false,
        layer: Layer::default(),
    }
}

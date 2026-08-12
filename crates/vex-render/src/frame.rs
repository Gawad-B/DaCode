//! What a frame consists of: rectangles and runs of text.
//!
//! Everything the UI layer wants on screen is expressed in these two primitives, in physical
//! pixels. Building a `Frame` involves no GPU work, so the UI layer can be written and tested
//! without a device.

use vex_theme::{FontSpec, Oklch};

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn is_visible(&self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }

    /// Inset on all sides. Negative values grow the rect.
    pub fn inset(self, by: f32) -> Self {
        Self::new(
            self.x + by,
            self.y + by,
            (self.width - by * 2.0).max(0.0),
            (self.height - by * 2.0).max(0.0),
        )
    }

    /// Splits a horizontal strip off the top, returning `(strip, remainder)` — the primitive
    /// the editor's vertical stack (title bar, tabs, body, statusline) is laid out with.
    pub fn split_top(self, height: f32) -> (Rect, Rect) {
        let height = height.min(self.height);
        (
            Rect::new(self.x, self.y, self.width, height),
            Rect::new(self.x, self.y + height, self.width, self.height - height),
        )
    }

    pub fn split_bottom(self, height: f32) -> (Rect, Rect) {
        let height = height.min(self.height);
        (
            Rect::new(self.x, self.bottom() - height, self.width, height),
            Rect::new(self.x, self.y, self.width, self.height - height),
        )
    }

    pub fn split_left(self, width: f32) -> (Rect, Rect) {
        let width = width.min(self.width);
        (
            Rect::new(self.x, self.y, width, self.height),
            Rect::new(self.x + width, self.y, self.width - width, self.height),
        )
    }

    pub fn split_right(self, width: f32) -> (Rect, Rect) {
        let width = width.min(self.width);
        (
            Rect::new(self.right() - width, self.y, width, self.height),
            Rect::new(self.x, self.y, self.width - width, self.height),
        )
    }
}

/// Paint order.
///
/// The renderer draws all rectangles and then all glyphs, which means a panel added later cannot
/// cover text added earlier — an overlay would be transparent. Layers fix that: each is drawn
/// completely (its quads, then its glyphs) before the next begins.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Layer {
    /// The editor itself: chrome, gutter, code.
    #[default]
    Base,
    /// Contextual panels that sit over the code — hover, completion, search results.
    Panel,
    /// Modal overlays that cover everything, like the command palette.
    Modal,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Layer::Base, Layer::Panel, Layer::Modal];
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub rect: Rect,
    /// Linear RGBA, already converted out of OKLCH.
    pub color: [f32; 4],
    pub layer: Layer,
}

/// The vex mark, drawn as a distance field rather than as rectangles.
///
/// A primitive of its own because the identity is not expressible in the other two: the strokes are
/// diagonal, so they need antialiasing, and the echo is translucent, so overlapping quads would
/// seam. See `mark.wgsl`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// The em box. Square; the mark is drawn inside its own 100x100 coordinate space.
    pub rect: Rect,
    /// Linear RGBA. Monochrome by rule — the mark never takes a mode hue, which is what lets it
    /// read as identity rather than state.
    pub color: [f32; 4],
    /// The echo's color, already mixed towards the surface it sits on.
    ///
    /// Resolved here rather than as an opacity in the shader: 34% alpha blended in linear light reads
    /// noticeably lighter than the same figure does in the browser the mark was designed in, and this
    /// palette mixes in OKLCH for exactly that reason.
    pub echo: [f32; 4],
    /// The single-chevron variant, for small sizes where the echo turns to mud.
    pub solo: bool,
    pub layer: Layer,
}

/// Below this many pixels the mark switches to the solo variant, as the handoff's PNG set does.
pub const SOLO_BELOW: f32 = 32.0;

/// Where the mark's leftmost ink falls, as a fraction of its em box.
///
/// Not 0.20 — the path starts at x=20, but the stroke is 11 wide and the arm is at 52 degrees, so the
/// outer edge of the left terminal reaches back to x≈15.7. Anything aligning to the mark's left edge
/// (the stacked lockup does) has to use the ink, not the path, or it sits visibly inboard.
pub const MARK_INK_LEFT: f32 = 0.1568;

/// Horizontal placement of a text run within its rect.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    /// Centred within the rect. Used only where there is nothing to align to — the empty state.
    Center,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub spec: FontSpec,
    pub color: Oklch,
    /// The box the run is laid out in; also its clip region.
    pub rect: Rect,
    pub align: Align,
    /// Vertically center within `rect`. Off for code, which sits on the row grid.
    pub center_vertically: bool,
    /// Wrap to `rect.width`. Prose in panels wraps; code never does — the editor computes soft
    /// wrap itself in display coordinates.
    pub wrap: bool,
    pub layer: Layer,
}

/// A complete description of one frame. Cleared and rebuilt each time.
#[derive(Debug, Default, Clone)]
pub struct Frame {
    pub quads: Vec<Quad>,
    pub texts: Vec<TextRun>,
    pub marks: Vec<Mark>,
    /// The layer subsequent drawing lands on.
    layer: Layer,
}

impl Frame {
    pub fn clear(&mut self) {
        self.quads.clear();
        self.texts.clear();
        self.marks.clear();
        self.layer = Layer::Base;
    }

    /// Sets the layer for everything drawn until it is set again.
    pub fn set_layer(&mut self, layer: Layer) {
        self.layer = layer;
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }

    pub fn quads_on(&self, layer: Layer) -> impl Iterator<Item = &Quad> {
        self.quads.iter().filter(move |quad| quad.layer == layer)
    }

    pub fn texts_on(&self, layer: Layer) -> impl Iterator<Item = &TextRun> {
        self.texts.iter().filter(move |run| run.layer == layer)
    }

    pub fn marks_on(&self, layer: Layer) -> impl Iterator<Item = &Mark> {
        self.marks.iter().filter(move |mark| mark.layer == layer)
    }

    /// Draws the mark, sized to fit `rect` and optically centred within it.
    ///
    /// Optically, not by bounding box: the echo makes the shape bottom-heavy, so centring the box
    /// leaves it looking low. The handoff's own PNGs carry the same correction.
    pub fn mark(&mut self, rect: Rect, color: Oklch, on: Oklch) {
        let size = rect.width.min(rect.height);
        if size <= 0.0 {
            return;
        }

        let solo = size < SOLO_BELOW;
        let box_rect = Rect::new(
            (rect.x + (rect.width - size) / 2.0).round(),
            // Up by 2% of the box, which is the nudge the handoff specifies.
            (rect.y + (rect.height - size) / 2.0 - size * 0.02).round(),
            size,
            size,
        );

        // 34% of the way from the surface to the ink: the handoff's echo opacity, read as a mix.
        let echo = on.mix(color, 0.34 * color.alpha).with_alpha(1.0);
        self.marks.push(Mark {
            rect: box_rect,
            color: color.to_linear(),
            echo: echo.to_linear(),
            solo,
            layer: self.layer,
        });
    }

    pub fn fill(&mut self, rect: Rect, color: Oklch) {
        self.fill_linear(rect, color.to_linear());
    }

    /// For colors already resolved to linear RGBA — the mode tints, which are composited
    /// against the canvas before they reach here.
    pub fn fill_linear(&mut self, rect: Rect, color: [f32; 4]) {
        if rect.is_visible() {
            self.quads.push(Quad {
                rect,
                color,
                layer: self.layer,
            });
        }
    }

    /// A horizontal gradient approximated by vertical strips. Used only for the gutter's mode
    /// wash; a real gradient would mean a second pipeline for one effect.
    pub fn fill_gradient_x(&mut self, rect: Rect, from: [f32; 4], to: [f32; 4], steps: usize) {
        let steps = steps.max(1);
        let step_width = rect.width / steps as f32;
        for i in 0..steps {
            let t = (i as f32 + 0.5) / steps as f32;
            let color = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
                from[2] + (to[2] - from[2]) * t,
                from[3] + (to[3] - from[3]) * t,
            ];
            self.fill_linear(
                Rect::new(
                    rect.x + step_width * i as f32,
                    rect.y,
                    step_width.ceil(),
                    rect.height,
                ),
                color,
            );
        }
    }

    /// A 1px rule. Kept as a named operation because the design uses hairlines structurally,
    /// and they must land on whole pixels or they turn grey.
    pub fn rule_top(&mut self, rect: Rect, color: Oklch, thickness: f32) {
        self.fill(
            Rect::new(rect.x, rect.y.round(), rect.width, thickness),
            color,
        );
    }

    pub fn rule_bottom(&mut self, rect: Rect, color: Oklch, thickness: f32) {
        self.fill(
            Rect::new(
                rect.x,
                (rect.bottom() - thickness).round(),
                rect.width,
                thickness,
            ),
            color,
        );
    }

    pub fn rule_left(&mut self, rect: Rect, color: Oklch, thickness: f32) {
        self.fill(
            Rect::new(rect.x.round(), rect.y, thickness, rect.height),
            color,
        );
    }

    pub fn text(&mut self, mut run: TextRun) {
        run.layer = self.layer;
        // The label role is always uppercase. Applied here rather than at every call site, so
        // a caller cannot accidentally ship a lowercase label.
        if run.spec.uppercase {
            run.text = run.text.to_uppercase();
        }
        if run.rect.is_visible() && !run.text.is_empty() {
            self.texts.push(run);
        }
    }
}

//! What a frame consists of: rectangles, shapes and runs of text.
//!
//! Everything the UI layer wants on screen is expressed in these primitives, in physical pixels. Building a `Frame` involves no GPU work, so the UI layer can be written and tested
//! without a device.

use crate::icon::{Icon, Stroke};
use dacode_theme::{FontSpec, Oklch};

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

    /// The overlap of two rects, empty when they do not touch.
    ///
    /// `Frame::fill` clips nothing — it only drops rects with no area — so anything positioned from
    /// a display column has to be measured against the code area before it is pushed, or a row
    /// scrolled sideways paints its selection over the line numbers.
    pub fn intersect(self, other: Rect) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self::new(
            x,
            y,
            (self.right().min(other.right()) - x).max(0.0),
            (self.bottom().min(other.bottom()) - y).max(0.0),
        )
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
    /// Corner radius in pixels. Zero keeps the edges exactly where the rectangle says, which is
    /// what a hairline needs to stay a single crisp row of pixels.
    pub radius: f32,
    /// Outline width in pixels; zero fills.
    pub border: f32,
}

/// An antialiased stroke or filled figure — the parts icons are made of.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Stroke width in pixels. Ignored by the filled kinds.
    pub width: f32,
    pub color: [f32; 4],
    pub layer: Layer,
}

/// Points in physical pixels; angles in radians, from +x and increasing clockwise on screen.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ShapeKind {
    Segment {
        from: [f32; 2],
        to: [f32; 2],
    },
    Ring {
        centre: [f32; 2],
        radius: f32,
    },
    Arc {
        centre: [f32; 2],
        radius: f32,
        start: f32,
        end: f32,
    },
    Disc {
        centre: [f32; 2],
        radius: f32,
    },
    Triangle {
        a: [f32; 2],
        b: [f32; 2],
        c: [f32; 2],
    },
}

/// The DaCode mark, drawn as a distance field rather than as rectangles.
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
    pub shapes: Vec<Shape>,
    /// The layer subsequent drawing lands on.
    layer: Layer,
    /// How see-through the window's backgrounds are: 0 is opaque. Kept across [`Frame::clear`],
    /// because it belongs to the window rather than to any one frame.
    transparency: f32,
}

impl Frame {
    pub fn clear(&mut self) {
        self.quads.clear();
        self.texts.clear();
        self.marks.clear();
        self.shapes.clear();
        self.layer = Layer::Base;
    }

    /// Sets how see-through backgrounds drawn with [`Frame::fill_backdrop`] are.
    pub fn set_transparency(&mut self, transparency: f32) {
        self.transparency = transparency.clamp(0.0, 0.9);
    }

    pub fn transparency(&self) -> f32 {
        self.transparency
    }

    pub fn shapes_on(&self, layer: Layer) -> impl Iterator<Item = &Shape> {
        self.shapes.iter().filter(move |shape| shape.layer == layer)
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
        self.push_quad(rect, color, 0.0, 0.0);
    }

    /// A window background: a surface the desktop may show through, when the window is translucent.
    ///
    /// Only surfaces go through this — panels, bars, the canvas behind the code. Text, highlights,
    /// the cursor and overlays stay solid, so a translucent editor is still a legible one. The
    /// renderer combines alpha by taking the larger, so two backdrops overlapping do not add up to a
    /// more opaque patch where they meet.
    pub fn fill_backdrop(&mut self, rect: Rect, color: Oklch) {
        let mut linear = color.to_linear();
        linear[3] *= 1.0 - self.transparency;
        self.push_quad(rect, linear, 0.0, 0.0);
    }

    /// A filled rectangle with rounded corners.
    pub fn fill_rounded(&mut self, rect: Rect, color: Oklch, radius: f32) {
        self.push_quad(rect, color.to_linear(), radius, 0.0);
    }

    /// A rounded rectangle's outline, `width` pixels wide, inside `rect`.
    pub fn outline_rounded(&mut self, rect: Rect, color: Oklch, radius: f32, width: f32) {
        self.push_quad(rect, color.to_linear(), radius, width.max(0.5));
    }

    fn push_quad(&mut self, rect: Rect, color: [f32; 4], radius: f32, border: f32) {
        if rect.is_visible() {
            self.quads.push(Quad {
                rect,
                color,
                layer: self.layer,
                radius: radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0),
                border,
            });
        }
    }

    /// Draws an icon inside `rect`, centred, at the largest square that fits.
    ///
    /// The stroke thins with the icon but never below a pixel and a quarter: thinner than that and
    /// antialiasing turns a line into a grey smudge.
    pub fn icon(&mut self, icon: Icon, rect: Rect, color: Oklch) {
        let size = rect.width.min(rect.height);
        if size <= 0.0 {
            return;
        }
        let unit = size / 24.0;
        let origin = [
            rect.x + (rect.width - size) / 2.0,
            rect.y + (rect.height - size) / 2.0,
        ];
        let at = |x: f32, y: f32| [origin[0] + x * unit, origin[1] + y * unit];
        let width = (1.8 * unit).max(1.25);
        let color = color.to_linear();

        for stroke in icon.strokes() {
            let kind = match stroke {
                Stroke::Line(x1, y1, x2, y2) => ShapeKind::Segment {
                    from: at(x1, y1),
                    to: at(x2, y2),
                },
                Stroke::Ring(x, y, r) => ShapeKind::Ring {
                    centre: at(x, y),
                    radius: r * unit,
                },
                Stroke::Arc(x, y, r, start, end) => ShapeKind::Arc {
                    centre: at(x, y),
                    radius: r * unit,
                    start: start.to_radians(),
                    end: end.to_radians(),
                },
                Stroke::Disc(x, y, r) => ShapeKind::Disc {
                    centre: at(x, y),
                    radius: r * unit,
                },
                Stroke::Triangle(x1, y1, x2, y2, x3, y3) => ShapeKind::Triangle {
                    a: at(x1, y1),
                    b: at(x2, y2),
                    c: at(x3, y3),
                },
            };
            self.shapes.push(Shape {
                kind,
                width,
                color,
                layer: self.layer,
            });
        }
    }

    /// A filled circle — an avatar, a badge, a status dot.
    pub fn disc(&mut self, centre: [f32; 2], radius: f32, color: Oklch) {
        self.shapes.push(Shape {
            kind: ShapeKind::Disc { centre, radius },
            width: 0.0,
            color: color.to_linear(),
            layer: self.layer,
        });
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

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

/// The DaCode mark: the letter ض, its fatha and its blocks, from the logo sheet.
///
/// A primitive of its own because it is artwork — two coverage masks the colours are applied to when
/// it is drawn, so it takes the theme's ink and keeps its amber. See `mark.wgsl`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// The em box. Square; the artwork is centred in it with a little air on every side.
    pub rect: Rect,
    /// The letter's colour, linear RGBA.
    pub ink: [f32; 4],
    /// The block's colour, linear RGBA.
    pub accent: [f32; 4],
    /// The small-size variant, where the two blocks become one.
    pub small: bool,
    pub layer: Layer,
}

/// A picture drawn into a rectangle: an image file, a PDF page. See `image.rs`.
#[derive(Debug, Clone)]
pub struct Image {
    pub rect: Rect,
    /// The part of the picture shown, as `x0, y0, x1, y1` fractions of it.
    pub uv: [f32; 4],
    pub picture: std::sync::Arc<crate::image::Picture>,
    pub opacity: f32,
    pub layer: Layer,
}

/// Below this many pixels the mark switches to the small variant, as the logo sheet specifies.
pub const SMALL_BELOW: f32 = 64.0;

/// Where the mark's leftmost ink falls, as a fraction of its em box. Anything aligning to the mark's
/// left edge — the welcome screen's lockup — aligns to the ink, not to the box.
pub const MARK_INK_LEFT: f32 = 0.029;

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
    /// A tighter clip than `rect`, when the run must be laid out in one box and shown only inside
    /// another — a row of code part-scrolled above the top of the code area keeps its position and
    /// loses the part over the tab bar. Set by [`Frame::text`] from [`Frame::set_clip`]; `None`
    /// clips to `rect` alone.
    pub clip: Option<Rect>,
}

/// A scale about a point and then a shift, applied to everything drawn while it is set: how an
/// overlay grows from its anchor as it arrives, or a page slides in, without each of its rectangles
/// being worked out a second time.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Zoom {
    pub origin: [f32; 2],
    pub factor: f32,
    pub shift: [f32; 2],
}

impl Zoom {
    pub fn about(origin: [f32; 2], factor: f32) -> Self {
        Self {
            origin,
            factor,
            shift: [0.0, 0.0],
        }
    }

    /// A move by `shift` and nothing else.
    pub fn shifted(shift: [f32; 2]) -> Self {
        Self {
            origin: [0.0, 0.0],
            factor: 1.0,
            shift,
        }
    }

    /// Whether this changes nothing, so drawing under it can skip the arithmetic.
    pub fn is_identity(&self) -> bool {
        self.factor == 1.0 && self.shift == [0.0, 0.0]
    }

    pub fn point(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + (x - self.origin[0]) * self.factor + self.shift[0],
            self.origin[1] + (y - self.origin[1]) * self.factor + self.shift[1],
        ]
    }

    pub fn rect(&self, rect: Rect) -> Rect {
        let [x, y] = self.point([rect.x, rect.y]);
        Rect::new(x, y, rect.width * self.factor, rect.height * self.factor)
    }
}

/// A complete description of one frame. Cleared and rebuilt each time.
#[derive(Debug, Default, Clone)]
pub struct Frame {
    pub quads: Vec<Quad>,
    pub texts: Vec<TextRun>,
    pub marks: Vec<Mark>,
    pub shapes: Vec<Shape>,
    pub images: Vec<Image>,
    /// The layer subsequent drawing lands on.
    layer: Layer,
    /// Where subsequent rectangles and text may appear, until it is cleared. See [`Frame::set_clip`].
    clip: Option<Rect>,
    /// How see-through the window's backgrounds are: 0 is opaque. Kept across [`Frame::clear`],
    /// because it belongs to the window rather than to any one frame.
    transparency: f32,
    /// How opaque subsequent drawing is, as a factor on every colour's alpha. `None` is fully
    /// opaque; kept as an option so the derived default is the opaque one.
    opacity: Option<f32>,
    /// A scale applied to subsequent drawing. See [`Frame::set_zoom`].
    zoom: Option<Zoom>,
}

impl Frame {
    pub fn clear(&mut self) {
        self.quads.clear();
        self.texts.clear();
        self.marks.clear();
        self.shapes.clear();
        self.images.clear();
        self.layer = Layer::Base;
        self.clip = None;
        self.opacity = None;
        self.zoom = None;
    }

    /// Multiplies the alpha of everything drawn from here on by `opacity`, until it is set again.
    ///
    /// How an overlay fades: one factor applied where colours enter the frame, rather than each of
    /// the dozens of colours in a card being faded by hand — the card that missed one would leave
    /// its scrim or its border behind on screen. At zero nothing is pushed at all.
    pub fn set_opacity(&mut self, opacity: f32) {
        let opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.opacity = (opacity < 1.0).then_some(opacity);
    }

    pub fn opacity(&self) -> f32 {
        self.opacity.unwrap_or(1.0)
    }

    /// Scales everything drawn from here on about a point, until it is set again; `None` lifts it.
    ///
    /// Text is scaled too, size and box together, so a run does not overflow a card that has shrunk
    /// around it. The clip is not: it is in the window's coordinates, not the card's.
    pub fn set_zoom(&mut self, zoom: Option<Zoom>) {
        self.zoom = zoom.filter(|zoom| {
            zoom.factor.is_finite()
                && zoom.shift.iter().all(|v| v.is_finite())
                && !zoom.is_identity()
        });
    }

    fn zoomed(&self, rect: Rect) -> Rect {
        self.zoom.map_or(rect, |zoom| zoom.rect(rect))
    }

    /// `color` with the frame's opacity applied, or `None` when that leaves nothing to see.
    fn faded(&self, mut color: [f32; 4]) -> Option<[f32; 4]> {
        color[3] *= self.opacity();
        (color[3] > 0.0 || self.opacity.is_none()).then_some(color)
    }

    /// Whether a shape's extent lies inside the clip. A shape cannot be cut the way a rectangle can,
    /// so one that pokes out is left out whole: a sliding panel's icons pop at its edge rather than
    /// being painted over the bar beside it.
    ///
    /// Conservative for an arc, which is bounded by its whole circle.
    fn inside_clip(&self, kind: &ShapeKind, width: f32) -> bool {
        let Some(clip) = self.clip else {
            return true;
        };
        let (min, max) = match *kind {
            ShapeKind::Segment { from, to } => (
                [from[0].min(to[0]), from[1].min(to[1])],
                [from[0].max(to[0]), from[1].max(to[1])],
            ),
            ShapeKind::Ring { centre, radius }
            | ShapeKind::Arc { centre, radius, .. }
            | ShapeKind::Disc { centre, radius } => (
                [centre[0] - radius, centre[1] - radius],
                [centre[0] + radius, centre[1] + radius],
            ),
            ShapeKind::Triangle { a, b, c } => (
                [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1])],
                [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])],
            ),
        };
        // Half the stroke falls outside the geometry; half a pixel of antialiasing is allowed.
        let slack = 0.5 - width / 2.0;
        min[0] >= clip.x - slack
            && min[1] >= clip.y - slack
            && max[0] <= clip.right() + slack
            && max[1] <= clip.bottom() + slack
    }

    fn push_shape(&mut self, kind: ShapeKind, width: f32, color: [f32; 4]) {
        let (kind, width) = match self.zoom {
            Some(zoom) => (
                match kind {
                    ShapeKind::Segment { from, to } => ShapeKind::Segment {
                        from: zoom.point(from),
                        to: zoom.point(to),
                    },
                    ShapeKind::Ring { centre, radius } => ShapeKind::Ring {
                        centre: zoom.point(centre),
                        radius: radius * zoom.factor,
                    },
                    ShapeKind::Arc {
                        centre,
                        radius,
                        start,
                        end,
                    } => ShapeKind::Arc {
                        centre: zoom.point(centre),
                        radius: radius * zoom.factor,
                        start,
                        end,
                    },
                    ShapeKind::Disc { centre, radius } => ShapeKind::Disc {
                        centre: zoom.point(centre),
                        radius: radius * zoom.factor,
                    },
                    ShapeKind::Triangle { a, b, c } => ShapeKind::Triangle {
                        a: zoom.point(a),
                        b: zoom.point(b),
                        c: zoom.point(c),
                    },
                },
                width * zoom.factor,
            ),
            None => (kind, width),
        };
        let Some(color) = self.faded(color) else {
            return;
        };
        if !self.inside_clip(&kind, width) {
            return;
        }
        self.shapes.push(Shape {
            kind,
            width,
            color,
            layer: self.layer,
        });
    }

    /// Confines subsequent rectangles and text to `clip`, until it is set again; `None` lifts it.
    ///
    /// For drawing that is positioned from something other than the box it must stay inside. Rows of
    /// code are placed from the scroll position, and the top one is partly above the code area while
    /// a touchpad has it part-scrolled: without a clip its line-number band and its glyphs would paint
    /// over the tab bar. Rectangles are cut to the clip; a run keeps its layout box and is clipped at
    /// draw time. An icon or shape is kept only if it lies wholly inside; marks are not affected.
    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
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

    /// Draws `picture` stretched to `rect`, cut to `within` and to the frame's clip. The cut is
    /// made in the picture's coordinates, so a page larger than the view shows the part of it that
    /// is in the view rather than being squeezed into it.
    pub fn image(
        &mut self,
        rect: Rect,
        picture: &std::sync::Arc<crate::image::Picture>,
        within: Rect,
    ) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let rect = self.zoomed(rect);
        let mut shown = rect.intersect(self.zoomed(within));
        if let Some(clip) = self.clip {
            shown = shown.intersect(clip);
        }
        if shown.width <= 0.0 || shown.height <= 0.0 {
            return;
        }
        let u = |x: f32| (x - rect.x) / rect.width;
        let v = |y: f32| (y - rect.y) / rect.height;
        self.images.push(Image {
            rect: shown,
            uv: [u(shown.x), v(shown.y), u(shown.right()), v(shown.bottom())],
            picture: picture.clone(),
            opacity: self.opacity(),
            layer: self.layer,
        });
    }

    /// Draws the mark, as large as fits in `rect` and centred in it: the letter in `ink`, the
    /// block in `accent`.
    pub fn mark(&mut self, rect: Rect, ink: Oklch, accent: Oklch) {
        let size = rect.width.min(rect.height);
        if size <= 0.0 {
            return;
        }
        let box_rect = Rect::new(
            (rect.x + (rect.width - size) / 2.0).round(),
            (rect.y + (rect.height - size) / 2.0).round(),
            size,
            size,
        );
        let (Some(ink), Some(accent)) =
            (self.faded(ink.to_linear()), self.faded(accent.to_linear()))
        else {
            return;
        };
        let box_rect = self.zoomed(box_rect);
        self.marks.push(Mark {
            rect: box_rect,
            ink,
            accent,
            small: box_rect.width < SMALL_BELOW,
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
        let Some(color) = self.faded(color) else {
            return;
        };
        let factor = self.zoom.map_or(1.0, |zoom| zoom.factor);
        let (rect, radius, border) = (self.zoomed(rect), radius * factor, border * factor);
        let rect = match self.clip {
            Some(clip) => rect.intersect(clip),
            None => rect,
        };
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
        self.icon_rotated(icon, rect, color, 0.0);
    }

    /// [`Frame::icon`], turned `radians` clockwise about the centre of the square it is drawn in.
    ///
    /// For the bell's wobble. The geometry is turned, not the pixels: a stroke stays a stroke at any
    /// angle, and an arc's angles turn with its centre so the bell's dome stays on the bell.
    pub fn icon_rotated(&mut self, icon: Icon, rect: Rect, color: Oklch, radians: f32) {
        let size = rect.width.min(rect.height);
        if size <= 0.0 {
            return;
        }
        let unit = size / 24.0;
        let origin = [
            rect.x + (rect.width - size) / 2.0,
            rect.y + (rect.height - size) / 2.0,
        ];
        let (sin, cos) = if radians == 0.0 {
            (0.0, 1.0)
        } else {
            radians.sin_cos()
        };
        // About the grid's centre, (12, 12). Exactly the unrotated arithmetic at zero, so an icon
        // that is not turning is not moved by rounding either.
        let at = |x: f32, y: f32| {
            let (dx, dy) = (x - 12.0, y - 12.0);
            let (x, y) = (12.0 + dx * cos - dy * sin, 12.0 + dx * sin + dy * cos);
            [origin[0] + x * unit, origin[1] + y * unit]
        };
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
                    start: start.to_radians() + radians,
                    end: end.to_radians() + radians,
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
            self.push_shape(kind, width, color);
        }
    }

    /// A filled circle — an avatar, a badge, a status dot.
    pub fn disc(&mut self, centre: [f32; 2], radius: f32, color: Oklch) {
        self.push_shape(ShapeKind::Disc { centre, radius }, 0.0, color.to_linear());
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
        if let Some(opacity) = self.opacity {
            run.color.alpha *= opacity;
            if run.color.alpha <= 0.0 {
                return;
            }
        }
        if let Some(zoom) = self.zoom {
            run.rect = zoom.rect(run.rect);
            run.clip = run.clip.map(|clip| zoom.rect(clip));
            // In quarter pixels, so the frames of a card growing share sizes: each new size is a
            // reshape of every run and new glyphs in the atlas, and a few hundredths of a pixel
            // between frames cannot be seen.
            run.spec.size_px = (run.spec.size_px * zoom.factor * 4.0).round() / 4.0;
        }
        if let Some(clip) = self.clip {
            let clip = run.clip.map_or(clip, |own| own.intersect(clip));
            // Wholly outside: there is nothing to shape, and nothing to draw.
            if !run.rect.intersect(clip).is_visible() {
                return;
            }
            run.clip = Some(clip);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ink() -> Oklch {
        Oklch::new(0.5, 0.1, 30.0)
    }

    fn points(shape: &Shape) -> Vec<[f32; 2]> {
        match shape.kind {
            ShapeKind::Segment { from, to } => vec![from, to],
            ShapeKind::Ring { centre, .. }
            | ShapeKind::Arc { centre, .. }
            | ShapeKind::Disc { centre, .. } => vec![centre],
            ShapeKind::Triangle { a, b, c } => vec![a, b, c],
        }
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn an_icon_turned_by_nothing_is_the_icon() {
        let rect = Rect::new(10.0, 20.0, 24.0, 24.0);
        let mut plain = Frame::default();
        plain.icon(Icon::Bell, rect, ink());
        let mut turned = Frame::default();
        turned.icon_rotated(Icon::Bell, rect, ink(), 0.0);
        assert_eq!(plain.shapes, turned.shapes);
    }

    #[test]
    fn a_rotated_icon_turns_about_its_centre() {
        // A half turn sends every point to its mirror through the centre. Rotating about a corner
        // instead — the easy mistake, since the grid's origin is the corner — would swing the bell
        // out of its button.
        let rect = Rect::new(10.0, 20.0, 24.0, 24.0);
        let centre = [22.0, 32.0];
        let mut plain = Frame::default();
        plain.icon(Icon::Close, rect, ink());
        let mut turned = Frame::default();
        turned.icon_rotated(Icon::Close, rect, ink(), std::f32::consts::PI);
        for (a, b) in plain.shapes.iter().zip(&turned.shapes) {
            for (p, q) in points(a).into_iter().zip(points(b)) {
                let mirrored = [2.0 * centre[0] - p[0], 2.0 * centre[1] - p[1]];
                assert!(close(mirrored, q), "{p:?} turned to {q:?}");
            }
        }
    }

    #[test]
    fn an_arc_turns_with_the_rest_of_its_icon() {
        let rect = Rect::new(0.0, 0.0, 24.0, 24.0);
        let mut plain = Frame::default();
        plain.icon(Icon::Bell, rect, ink());
        let mut turned = Frame::default();
        turned.icon_rotated(Icon::Bell, rect, ink(), 0.25);
        let arcs = |frame: &Frame| -> Vec<(f32, f32)> {
            frame
                .shapes
                .iter()
                .filter_map(|shape| match shape.kind {
                    ShapeKind::Arc { start, end, .. } => Some((start, end)),
                    _ => None,
                })
                .collect()
        };
        for ((s0, e0), (s1, e1)) in arcs(&plain).into_iter().zip(arcs(&turned)) {
            assert!((s1 - s0 - 0.25).abs() < 1e-5 && (e1 - e0 - 0.25).abs() < 1e-5);
        }
    }

    #[test]
    fn opacity_scales_every_alpha_and_zero_draws_nothing() {
        let mut frame = Frame::default();
        frame.set_opacity(0.5);
        frame.fill(Rect::new(0.0, 0.0, 10.0, 10.0), ink());
        frame.icon(Icon::Close, Rect::new(0.0, 0.0, 24.0, 24.0), ink());
        frame.text(TextRun {
            text: "x".into(),
            spec: dacode_theme::Typography::default().spec(dacode_theme::Role::Ui),
            color: ink(),
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            align: Align::Left,
            center_vertically: false,
            wrap: false,
            layer: Layer::Base,
            clip: None,
        });
        assert!((frame.quads[0].color[3] - 0.5).abs() < 1e-6);
        assert!(frame.shapes.iter().all(|s| (s.color[3] - 0.5).abs() < 1e-6));
        assert!((frame.texts[0].color.alpha - 0.5).abs() < 1e-6);

        // Fully faded is not drawn at all: a transparent quad still costs an instance.
        let mut frame = Frame::default();
        frame.set_opacity(0.0);
        frame.fill(Rect::new(0.0, 0.0, 10.0, 10.0), ink());
        frame.icon(Icon::Close, Rect::new(0.0, 0.0, 24.0, 24.0), ink());
        assert!(frame.quads.is_empty() && frame.shapes.is_empty());
    }

    #[test]
    fn a_zoom_scales_what_is_drawn_about_its_origin_and_clear_forgets_it() {
        let mut frame = Frame::default();
        frame.set_zoom(Some(Zoom::about([100.0, 100.0], 0.5)));
        frame.fill(Rect::new(100.0, 100.0, 40.0, 20.0), ink());
        assert_eq!(frame.quads[0].rect, Rect::new(100.0, 100.0, 20.0, 10.0));
        frame.clear();
        frame.fill(Rect::new(100.0, 100.0, 40.0, 20.0), ink());
        assert_eq!(frame.quads[0].rect, Rect::new(100.0, 100.0, 40.0, 20.0));
    }

    #[test]
    fn a_clip_keeps_icons_inside_it() {
        // The sidebar slides in under a clip; an icon that was not clipped would be drawn over the
        // activity bar beside it.
        let mut frame = Frame::default();
        frame.set_clip(Some(Rect::new(50.0, 0.0, 100.0, 100.0)));
        frame.icon(Icon::Close, Rect::new(0.0, 0.0, 24.0, 24.0), ink());
        assert!(frame.shapes.is_empty(), "wholly outside");
        frame.icon(Icon::Close, Rect::new(60.0, 10.0, 24.0, 24.0), ink());
        assert!(!frame.shapes.is_empty(), "wholly inside");
    }
}

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

#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub rect: Rect,
    /// Linear RGBA, already converted out of OKLCH.
    pub color: [f32; 4],
}

/// Horizontal placement of a text run within its rect.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
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
}

/// A complete description of one frame. Cleared and rebuilt each time.
#[derive(Debug, Default, Clone)]
pub struct Frame {
    pub quads: Vec<Quad>,
    pub texts: Vec<TextRun>,
}

impl Frame {
    pub fn clear(&mut self) {
        self.quads.clear();
        self.texts.clear();
    }

    pub fn fill(&mut self, rect: Rect, color: Oklch) {
        self.fill_linear(rect, color.to_linear());
    }

    /// For colors already resolved to linear RGBA — the mode tints, which are composited
    /// against the canvas before they reach here.
    pub fn fill_linear(&mut self, rect: Rect, color: [f32; 4]) {
        if rect.is_visible() {
            self.quads.push(Quad { rect, color });
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

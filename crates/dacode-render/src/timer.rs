//! Frame timing.
//!
//! The design puts measured frame time in the statusline against a 16.6ms budget, and that is
//! the right instinct: a performance target you cannot see is a target you will lose. This is
//! infrastructure, not decoration — the number has to be real from the first frame.
//!
//! **It brackets the whole frame**: building the [`crate::Frame`] *and* submitting it. It used to
//! start inside `Renderer::render`, which excluded the build — the half this project has optimised
//! hardest, where the sidebar walking 512 rows to compute one coordinate lived, and the half every
//! remaining performance defect lands in. A budget that cannot see the work is not a budget, and
//! the number it showed was mostly a vsync wait nobody controls.

use dacode_theme::FRAME_BUDGET_MS;
use std::time::{Duration, Instant};

const WINDOW: usize = 120;

pub struct FrameTimer {
    started: Option<Instant>,
    samples: [f32; WINDOW],
    next: usize,
    filled: usize,
    over_budget: u64,
    total: u64,
}

impl Default for FrameTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameTimer {
    pub fn new() -> Self {
        Self {
            started: None,
            samples: [0.0; WINDOW],
            next: 0,
            filled: 0,
            over_budget: 0,
            total: 0,
        }
    }

    pub fn begin(&mut self) {
        self.started = Some(Instant::now());
    }

    pub fn end(&mut self) {
        let Some(started) = self.started.take() else {
            return;
        };
        self.record(started.elapsed());
    }

    fn record(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f32() * 1000.0;
        self.samples[self.next] = ms;
        self.next = (self.next + 1) % WINDOW;
        self.filled = (self.filled + 1).min(WINDOW);
        self.total += 1;
        if ms > FRAME_BUDGET_MS {
            self.over_budget += 1;
        }
    }

    fn window(&self) -> &[f32] {
        &self.samples[..self.filled]
    }

    /// Mean over the recent window. What the statusline shows.
    pub fn average_ms(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        self.window().iter().sum::<f32>() / self.filled as f32
    }

    /// Worst recent frame. More useful than the mean for judging whether typing feels smooth:
    /// one 40ms frame is noticed, a good average is not.
    pub fn worst_ms(&self) -> f32 {
        self.window().iter().copied().fold(0.0, f32::max)
    }

    pub fn is_over_budget(&self) -> bool {
        self.average_ms() > FRAME_BUDGET_MS
    }

    /// Share of all frames that missed the budget, since startup.
    pub fn miss_ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.over_budget as f32 / self.total as f32
    }

    pub fn frames(&self) -> u64 {
        self.total
    }
}

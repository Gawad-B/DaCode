//! Text shaping and rasterization, on top of cosmic-text + glyphon.
//!
//! Shaping is the part of an editor nobody should write by hand: ligatures, combining marks,
//! bidi, font fallback. What this module owns instead is *not reshaping needlessly* — shaped
//! buffers are cached across frames, because reshaping every visible line every frame is
//! exactly the kind of unbounded per-frame work the 60fps contract forbids.

use crate::frame::{Align, TextRun};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family as CtFamily, FontSystem, Metrics, Resolution, Shaping,
    Style as CtStyle, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
    Weight as CtWeight,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use vex_theme::{Family, Style};

/// Frames a shaped buffer may go unused before it is dropped. Generous: re-shaping a line the
/// user scrolled past and came back to is worse than holding a few hundred KB.
const CACHE_TTL_FRAMES: u64 = 600;

struct Cached {
    buffer: Buffer,
    /// Widest laid-out line, needed for right alignment.
    width: f32,
    last_used: u64,
}

/// Which concrete font family name to use for each of the design's three roles, resolved once
/// against the installed fonts.
struct ResolvedFamilies {
    serif: FamilyChoice,
    mono: FamilyChoice,
    sans: FamilyChoice,
}

enum FamilyChoice {
    Named(String),
    Generic(&'static str),
}

impl FamilyChoice {
    fn as_ct(&self) -> CtFamily<'_> {
        match self {
            FamilyChoice::Named(name) => CtFamily::Name(name),
            FamilyChoice::Generic("serif") => CtFamily::Serif,
            FamilyChoice::Generic("monospace") => CtFamily::Monospace,
            _ => CtFamily::SansSerif,
        }
    }

    fn describe(&self) -> &str {
        match self {
            FamilyChoice::Named(name) => name,
            FamilyChoice::Generic(g) => g,
        }
    }
}

pub struct TextStack {
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
    buffers: HashMap<u64, Cached>,
    families: ResolvedFamilies,
    frame: u64,
}

impl TextStack {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut font_system = FontSystem::new();
        let families = ResolvedFamilies {
            serif: resolve(&mut font_system, Family::Serif, "serif"),
            mono: resolve(&mut font_system, Family::Mono, "monospace"),
            sans: resolve(&mut font_system, Family::Sans, "sans-serif"),
        };

        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        Self {
            font_system,
            swash: SwashCache::new(),
            atlas,
            viewport,
            renderer,
            buffers: HashMap::new(),
            families,
            frame: 0,
        }
    }

    /// The faces actually in use — worth surfacing, since a silently-substituted serif is the
    /// difference between the intended design and something that merely resembles it.
    pub fn resolved_families(&self) -> [&str; 3] {
        [
            self.families.serif.describe(),
            self.families.mono.describe(),
            self.families.sans.describe(),
        ]
    }

    fn family_for(&self, family: Family) -> CtFamily<'_> {
        match family {
            Family::Serif => self.families.serif.as_ct(),
            Family::Mono => self.families.mono.as_ct(),
            Family::Sans => self.families.sans.as_ct(),
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        runs: &[TextRun],
    ) -> anyhow::Result<()> {
        self.frame += 1;
        self.viewport.update(queue, Resolution { width, height });

        // Pass 1: shape anything not already cached. Separated from pass 2 so the borrow
        // checker can hand `prepare` a mutable font system and immutable buffers at once.
        let mut keys = Vec::with_capacity(runs.len());
        for run in runs {
            let key = run_key(run);
            keys.push(key);

            if let Some(cached) = self.buffers.get_mut(&key) {
                cached.last_used = self.frame;
                continue;
            }

            let metrics = Metrics::new(run.spec.size_px, run.spec.size_px * run.spec.line_height);
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            if run.wrap {
                // Prose in panels wraps to its box.
                buffer.set_size(Some(run.rect.width), None);
            } else {
                // Code does not: soft wrap is the *editor's* decision, computed in display
                // coordinates, not something the shaper should improvise per run.
                buffer.set_size(None, None);
            }

            let attrs = Attrs::new()
                .family(self.family_for(run.spec.family))
                .weight(CtWeight(run.spec.weight))
                .style(match run.spec.style {
                    Style::Roman => CtStyle::Normal,
                    Style::Italic => CtStyle::Italic,
                })
                // cosmic-text adds this to a *normalized* advance, so the unit is em — not px.
                // Scaling it by the font size collapses tight tracking into overlapping glyphs
                // and blows loose tracking apart by an order of magnitude.
                .letter_spacing(run.spec.tracking_em);

            buffer.set_text(&run.text, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);

            let width = buffer
                .layout_runs()
                .map(|line| line.line_w)
                .fold(0.0_f32, f32::max);

            self.buffers.insert(
                key,
                Cached {
                    buffer,
                    width,
                    last_used: self.frame,
                },
            );
        }

        // Pass 2: place each shaped buffer.
        let Self {
            font_system,
            swash,
            atlas,
            viewport,
            renderer,
            buffers,
            ..
        } = self;

        let areas = runs.iter().zip(&keys).filter_map(|(run, key)| {
            let cached = buffers.get(key)?;
            let line_height = run.spec.size_px * run.spec.line_height;

            let left = match run.align {
                Align::Left => run.rect.x,
                Align::Right => run.rect.right() - cached.width,
            };
            let top = if run.center_vertically {
                run.rect.y + (run.rect.height - line_height) / 2.0
            } else {
                run.rect.y
            };

            let [r, g, b, a] = run.color.to_srgb8();

            Some(TextArea {
                buffer: &cached.buffer,
                left: left.round(),
                top: top.round(),
                scale: 1.0,
                bounds: TextBounds {
                    left: run.rect.x.floor() as i32,
                    top: run.rect.y.floor() as i32,
                    right: run.rect.right().ceil() as i32,
                    bottom: run.rect.bottom().ceil() as i32,
                },
                default_color: Color::rgba(r, g, b, a),
                custom_glyphs: &[],
            })
        });

        renderer.prepare(device, queue, font_system, atlas, viewport, areas, swash)?;

        self.evict();
        Ok(())
    }

    fn evict(&mut self) {
        let frame = self.frame;
        self.buffers
            .retain(|_, cached| frame.saturating_sub(cached.last_used) < CACHE_TTL_FRAMES);
    }

    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) -> anyhow::Result<()> {
        self.renderer.render(&self.atlas, &self.viewport, pass)?;
        Ok(())
    }

    pub fn trim_atlas(&mut self) {
        self.atlas.trim();
    }
}

/// Picks the first installed family from the design's preference list, falling back to a
/// generic. Done by inspection rather than by relying on the shaper's fallback, so the editor
/// can report what it actually got.
fn resolve(font_system: &mut FontSystem, family: Family, generic: &'static str) -> FamilyChoice {
    let installed: Vec<String> = font_system
        .db()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();

    for candidate in family.candidates() {
        if installed.iter().any(|name| name == candidate) {
            return FamilyChoice::Named((*candidate).to_string());
        }
    }
    FamilyChoice::Generic(generic)
}

fn run_key(run: &TextRun) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    run.text.hash(&mut hasher);
    run.spec.family.hash(&mut hasher);
    run.spec.style.hash(&mut hasher);
    run.spec.weight.hash(&mut hasher);
    run.spec.size_px.to_bits().hash(&mut hasher);
    run.spec.line_height.to_bits().hash(&mut hasher);
    run.spec.tracking_em.to_bits().hash(&mut hasher);
    run.wrap.hash(&mut hasher);
    if run.wrap {
        // Wrapped runs are shaped against their box, so the width is part of their identity.
        run.rect.width.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

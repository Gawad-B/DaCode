//! Text shaping and rasterization, on top of cosmic-text + glyphon.
//!
//! Shaping is the part of an editor nobody should write by hand: ligatures, combining marks,
//! bidi, font fallback. What this module owns instead is *not reshaping needlessly* — shaped
//! buffers are cached across frames, because reshaping every visible line every frame is
//! exactly the kind of unbounded per-frame work the 60fps contract forbids.

use crate::frame::{Align, Layer, TextRun};
use dacode_theme::Style;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family as CtFamily, FontSystem, Metrics, Resolution, Shaping,
    Style as CtStyle, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
    Weight as CtWeight,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The bundled face: Fira Code, and nothing else.
///
/// Embedded in the binary rather than fetched or installed, so the first frame never waits on a font
/// and the editor looks like itself on a machine where nothing is installed. SIL OFL; the licence
/// text sits beside it.
///
/// One face for everything — the code, the chrome, the headings. There were three: a serif for
/// headings and the wordmark and a sans for labels beside the code face. The contrast was the
/// design's idea; one familiar face everywhere is what people asked for, and it is also what makes
/// every width the chrome estimates exact, because a monospaced face has one advance.
mod bundled {
    pub const FIRA_CODE: &[u8] = include_bytes!("../fonts/FiraCode-Regular.ttf");
}

/// Frames a shaped buffer may go unused before it is dropped. Generous: re-shaping a line the
/// user scrolled past and came back to is worse than holding a few hundred KB.
const CACHE_TTL_FRAMES: u64 = 600;

struct Cached {
    buffer: Buffer,
    /// Widest laid-out line, needed for right alignment.
    width: f32,
    last_used: u64,
    /// What was shaped, so a hash collision cannot serve the wrong text.
    ///
    /// The key is 64 bits of `DefaultHasher` and nothing else, which is fine as an index and
    /// worthless as proof: two different runs that collide would have rendered *one another's*
    /// glyphs, silently, and only on the machine and the document where the collision happened.
    /// The comparison costs a string equality on a cache hit — nothing beside the shaping it
    /// avoids — and it turns an unreproducible visual corruption into an ordinary re-shape.
    identity: Identity,
}

/// Everything that decides how a run is shaped. Compared on a cache hit.
#[derive(PartialEq)]
struct Identity {
    text: String,
    spec: dacode_theme::FontSpec,
    wrap: bool,
    /// Only meaningful when `wrap` is set; a wrapped run is shaped against its box.
    width: f32,
}

impl Identity {
    fn of(run: &TextRun) -> Self {
        Self {
            text: run.text.clone(),
            spec: run.spec,
            wrap: run.wrap,
            width: if run.wrap { run.rect.width } else { 0.0 },
        }
    }
}

enum FamilyChoice {
    Named(String),
    Generic(&'static str),
}

impl FamilyChoice {
    fn as_ct(&self) -> CtFamily<'_> {
        match self {
            FamilyChoice::Named(name) => CtFamily::Name(name),
            FamilyChoice::Generic(_) => CtFamily::Monospace,
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
    /// One renderer per layer: `prepare` overwrites a renderer's vertex buffer, so layers that must
    /// be drawn at different points in the pass cannot share one.
    renderers: Vec<(Layer, TextRenderer)>,
    buffers: HashMap<u64, Cached>,
    /// The face in use, resolved once against what is installed.
    face: FamilyChoice,
    frame: u64,
}

impl TextStack {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut font_system = load_fonts();
        let face = resolve(&mut font_system);

        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderers = Layer::ALL
            .into_iter()
            .map(|layer| {
                (
                    layer,
                    TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None),
                )
            })
            .collect();

        Self {
            font_system,
            swash: SwashCache::new(),
            atlas,
            viewport,
            renderers,
            buffers: HashMap::new(),
            face,
            frame: 0,
        }
    }

    /// The face actually in use — worth surfacing, since a silently substituted face is the
    /// difference between the intended design and something that merely resembles it.
    pub fn resolved_face(&self) -> &str {
        self.face.describe()
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

            let identity = Identity::of(run);
            match self.buffers.get_mut(&key) {
                // The identity check is what makes the key safe to be only 64 bits wide.
                Some(cached) if cached.identity == identity => {
                    cached.last_used = self.frame;
                    continue;
                }
                // A collision: two different runs hashed the same. Re-shape and take the slot.
                Some(_) => {
                    self.buffers.remove(&key);
                }
                None => {}
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
                .family(self.face.as_ct())
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
                    identity,
                },
            );
        }

        // Pass 2: place each shaped buffer, one prepare per layer.
        let Self {
            font_system,
            swash,
            atlas,
            viewport,
            renderers,
            buffers,
            ..
        } = self;

        let place = |run: &TextRun, key: &u64| -> Option<TextArea<'_>> {
            let cached = buffers.get(key)?;
            let line_height = run.spec.size_px * run.spec.line_height;

            let left = match run.align {
                Align::Left => run.rect.x,
                Align::Right => run.rect.right() - cached.width,
                Align::Center => run.rect.x + (run.rect.width - cached.width) / 2.0,
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
                bounds: {
                    let shown = run.clip.map_or(run.rect, |clip| run.rect.intersect(clip));
                    // Outward rounding is right for a run's own box, but a top cut by a clip must
                    // round inward, or a part-scrolled row reaches a pixel over the strip above it.
                    let top = if shown.y > run.rect.y {
                        shown.y.ceil()
                    } else {
                        shown.y.floor()
                    };
                    TextBounds {
                        left: shown.x.floor() as i32,
                        top: top as i32,
                        right: shown.right().ceil() as i32,
                        bottom: shown.bottom().ceil() as i32,
                    }
                },
                default_color: Color::rgba(r, g, b, a),
                custom_glyphs: &[],
            })
        };

        for (layer, renderer) in renderers.iter_mut() {
            let areas = runs
                .iter()
                .zip(&keys)
                .filter(|(run, _)| run.layer == *layer)
                .filter_map(|(run, key)| place(run, key));

            renderer.prepare(device, queue, font_system, atlas, viewport, areas, swash)?;
        }

        self.evict();
        Ok(())
    }

    fn evict(&mut self) {
        let frame = self.frame;
        self.buffers
            .retain(|_, cached| frame.saturating_sub(cached.last_used) < CACHE_TTL_FRAMES);
    }

    /// Draws one layer's glyphs.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>, layer: Layer) -> anyhow::Result<()> {
        if let Some((_, renderer)) = self.renderers.iter().find(|(l, _)| *l == layer) {
            renderer.render(&self.atlas, &self.viewport, pass)?;
        }
        Ok(())
    }

    pub fn trim_atlas(&mut self) {
        self.atlas.trim();
    }
}

/// Builds the font system: bundled faces first, then the system's.
///
/// Order matters for startup. The bundled faces parse from memory in well under a millisecond, so the
/// editor can draw its own design immediately. The system scan follows because it is the only source
/// of fallback coverage — CJK, emoji, anything the bundled families do not contain — and a missing
/// glyph should degrade to a substitute rather than to a blank box.
fn load_fonts() -> FontSystem {
    let started = std::time::Instant::now();

    let mut db = fontdb::Database::new();
    db.load_font_data(bundled::FIRA_CODE.to_vec());
    let bundled_count = db.len();
    let bundled_time = started.elapsed();

    // The system scan costs ~150ms and buys fallback coverage — CJK, emoji, scripts Fira Code does
    // not draw — that one bundled face cannot provide.
    // Correctness wins by default, but a user who only ever edits Latin text can have the startup
    // back — the cost is real and this is the one place it is paid.
    let skip_system = std::env::var_os("DACODE_NO_SYSTEM_FONTS").is_some();
    let system_started = std::time::Instant::now();
    if !skip_system {
        db.load_system_fonts();
    }
    let system_time = system_started.elapsed();

    // Every generic name resolves to the one face, so nothing asking for "sans-serif" reaches for
    // whatever the system considers one and comes back in a different face from everything else.
    db.set_serif_family("Fira Code");
    db.set_monospace_family("Fira Code");
    db.set_sans_serif_family("Fira Code");

    if std::env::var_os("DACODE_FONT_TRACE").is_some() {
        eprintln!(
            "dacode: fonts — {bundled_count} bundled in {:.2}ms, {} system in {:.0}ms",
            bundled_time.as_secs_f32() * 1000.0,
            db.len() - bundled_count,
            system_time.as_secs_f32() * 1000.0
        );
    }

    let locale = std::env::var("LANG")
        .ok()
        .and_then(|lang| lang.split('.').next().map(str::to_string))
        .unwrap_or_else(|| "en-US".to_string());

    FontSystem::new_with_locale_and_db(locale, db)
}

/// Picks the first installed face from the preference list, falling back to the generic monospace.
/// Done by inspection rather than by relying on the shaper's fallback, so the editor can report
/// what it actually got.
fn resolve(font_system: &mut FontSystem) -> FamilyChoice {
    let installed: Vec<String> = font_system
        .db()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();

    for candidate in dacode_theme::FACES {
        if installed.iter().any(|name| name == candidate) {
            return FamilyChoice::Named((*candidate).to_string());
        }
    }
    FamilyChoice::Generic("monospace")
}

fn run_key(run: &TextRun) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    run.text.hash(&mut hasher);
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

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
///
/// Only the Regular weight is bundled, so every run is shaped at a weight that face has — see
/// [`shaped_weight`]. Asking for one it lacks does not get a bolder Fira Code: it gets a different
/// family.
mod bundled {
    pub const FIRA_CODE: &[u8] = include_bytes!("../fonts/FiraCode-Regular.ttf");
}

/// Whether the bundled face has a glyph for `ch`.
///
/// A character it lacks is drawn from whatever installed font has one, so a chrome string holding
/// one is a word with a letter in another typeface. The tests that guard the interface's own strings
/// ask this; the user's file is allowed to fall back, since a CJK comment or an emoji in a string
/// literal is better drawn in some face than not drawn at all.
pub fn fira_code_draws(ch: char) -> bool {
    static FACE: std::sync::OnceLock<glyphon::Font> = std::sync::OnceLock::new();
    FACE.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_font_data(bundled::FIRA_CODE.to_vec());
        let id = db.faces().next().expect("the bundled face parses").id;
        glyphon::Font::new(&db, id, fontdb::Weight::NORMAL).expect("the bundled face loads")
    })
    .as_swash()
    .charmap()
    .map(ch)
        != 0
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
    /// The weights the bundled faces come in; every run is shaped at one of them.
    weights: Vec<u16>,
    frame: u64,
    /// The bundled faces' ids, and the substitutions already reported, when `DACODE_FONT_TRACE` is
    /// set. A glyph drawn from any other face is a place the interface stopped being Fira Code.
    trace: Option<Trace>,
}

struct Trace {
    bundled: Vec<fontdb::ID>,
    seen: std::collections::HashSet<(String, u16, Style, char)>,
}

impl TextStack {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let (mut font_system, bundled) = load_fonts();
        let face = resolve(&mut font_system);
        let weights = bundled_weights(font_system.db(), &bundled);
        let trace = std::env::var_os("DACODE_FONT_TRACE").map(|_| Trace {
            bundled,
            seen: Default::default(),
        });

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
            weights,
            frame: 0,
            trace,
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

            let attrs = attrs(self.face.as_ct(), &run.spec, &self.weights);
            buffer.set_text(&run.text, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            if let Some(trace) = &mut self.trace {
                trace.record(&self.font_system, &buffer, run);
            }

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

/// What a run is shaped with.
///
/// The weight is the nearest the bundled faces have, never the one asked for. cosmic-text takes the
/// requested family only from a face of *exactly* the requested weight (or a variable face whose
/// axis covers it); otherwise it moves on to its platform fallback list, and `Noto Sans` at 600 is
/// what came back. So every settings heading, every row name, every uppercase label and the wordmark
/// were set in a sans on any machine with fonts installed, and in Fira Code in every screenshot,
/// because `DACODE_NO_SYSTEM_FONTS` left nothing to fall back to. Snapping the weight here keeps the
/// family whatever a caller asks for; [`dacode_theme::Typography`] asks only for weights that exist,
/// so nothing is snapped in practice and the type scale tells the truth about what is drawn.
///
/// Italic needs no such help: style is not part of that exact-match test, so an italic request
/// keeps Fira Code and cosmic-text slants it (its synthetic `FAKE_ITALIC`), the face having none.
fn attrs<'a>(family: CtFamily<'a>, spec: &dacode_theme::FontSpec, weights: &[u16]) -> Attrs<'a> {
    Attrs::new()
        .family(family)
        .weight(CtWeight(shaped_weight(spec.weight, weights)))
        .style(match spec.style {
            Style::Roman => CtStyle::Normal,
            Style::Italic => CtStyle::Italic,
        })
        // cosmic-text adds this to a *normalized* advance, so the unit is em — not px.
        // Scaling it by the font size collapses tight tracking into overlapping glyphs
        // and blows loose tracking apart by an order of magnitude.
        .letter_spacing(spec.tracking_em)
}

/// The bundled weight nearest `requested`, the lighter on a tie.
fn shaped_weight(requested: u16, weights: &[u16]) -> u16 {
    weights
        .iter()
        .copied()
        .min_by_key(|weight| (weight.abs_diff(requested), *weight))
        .unwrap_or(requested)
}

/// The weights the bundled faces come in.
fn bundled_weights(db: &fontdb::Database, bundled: &[fontdb::ID]) -> Vec<u16> {
    let mut weights: Vec<u16> = bundled
        .iter()
        .filter_map(|id| db.face(*id))
        .map(|face| face.weight.0)
        .collect();
    weights.sort_unstable();
    weights.dedup();
    weights
}

impl Trace {
    /// Reports, once each, every character a run drew from a face that is not bundled.
    fn record(&mut self, font_system: &FontSystem, buffer: &Buffer, run: &TextRun) {
        for line in buffer.layout_runs() {
            for glyph in line.glyphs {
                if self.bundled.contains(&glyph.font_id) {
                    continue;
                }
                let family = font_system
                    .db()
                    .face(glyph.font_id)
                    .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
                    .unwrap_or_else(|| "?".to_string());
                let text = &line.text[glyph.start..glyph.end];
                for ch in text.chars() {
                    let key = (family.clone(), run.spec.weight, run.spec.style, ch);
                    if self.seen.insert(key) {
                        eprintln!(
                            "dacode: substituted {family:?} for {ch:?} (U+{:04X}) at weight {} {:?} in {:?}",
                            ch as u32, run.spec.weight, run.spec.style, run.text
                        );
                    }
                }
            }
        }
    }
}

/// Builds the font system: bundled faces first, then the system's. Returns the bundled faces' ids
/// beside it, since nothing in the database itself says which faces were embedded.
///
/// Order matters for startup. The bundled faces parse from memory in well under a millisecond, so the
/// editor can draw its own design immediately. The system scan follows because it is the only source
/// of fallback coverage — CJK, emoji, anything the bundled families do not contain — and a missing
/// glyph should degrade to a substitute rather than to a blank box.
fn load_fonts() -> (FontSystem, Vec<fontdb::ID>) {
    let started = std::time::Instant::now();

    let mut db = fontdb::Database::new();
    db.load_font_data(bundled::FIRA_CODE.to_vec());
    let bundled_count = db.len();
    let bundled: Vec<fontdb::ID> = db.faces().map(|face| face.id).collect();
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

    (FontSystem::new_with_locale_and_db(locale, db), bundled)
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

#[cfg(test)]
mod tests {
    use super::*;
    use dacode_theme::{Role, Typography};
    use glyphon::cosmic_text::{Fallback, PlatformFallback};

    fn bundled_only() -> (fontdb::Database, Vec<fontdb::ID>) {
        let mut db = fontdb::Database::new();
        db.load_font_data(bundled::FIRA_CODE.to_vec());
        let ids = db.faces().map(|face| face.id).collect();
        (db, ids)
    }

    /// The bundled face, plus a stand-in for every family cosmic-text falls back to on this
    /// platform, at every weight and in both styles.
    ///
    /// The stand-ins are what made the defect invisible in tests and screenshots: with only the
    /// bundled face loaded there is nothing to fall back *to*, so a weight Fira Code lacks still came
    /// out in Fira Code. A real machine has `Noto Sans` at 600 (or `Segoe UI`, or whatever the
    /// platform's list names), and cosmic-text takes it. These carry Fira Code's own outlines under
    /// another name, so they cover every character the bundled face does and nothing but the family
    /// can tell them apart.
    fn with_decoys() -> (FontSystem, Vec<fontdb::ID>) {
        let (mut db, bundled) = bundled_only();
        let data: std::sync::Arc<dyn AsRef<[u8]> + Send + Sync> =
            std::sync::Arc::new(bundled::FIRA_CODE.to_vec());
        for family in PlatformFallback.common_fallback() {
            for weight in (100..=900).step_by(100) {
                for style in [fontdb::Style::Normal, fontdb::Style::Italic] {
                    db.push_face_info(fontdb::FaceInfo {
                        id: fontdb::ID::dummy(),
                        source: fontdb::Source::Binary(data.clone()),
                        index: 0,
                        families: vec![(
                            family.to_string(),
                            fontdb::Language::English_UnitedStates,
                        )],
                        post_script_name: format!("Decoy-{weight}-{style:?}"),
                        style,
                        weight: fontdb::Weight(weight),
                        stretch: fontdb::Stretch::Normal,
                        monospaced: false,
                    });
                }
            }
        }
        db.set_serif_family("Fira Code");
        db.set_monospace_family("Fira Code");
        db.set_sans_serif_family("Fira Code");
        (
            FontSystem::new_with_locale_and_db("en-US".into(), db),
            bundled,
        )
    }

    /// Shapes `text` with `attrs` and returns the face of every glyph.
    fn faces_used(font_system: &mut FontSystem, text: &str, attrs: &Attrs) -> Vec<fontdb::ID> {
        let mut buffer = Buffer::new(font_system, Metrics::new(14.0, 20.0));
        buffer.set_size(None, None);
        buffer.set_text(text, attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);
        buffer
            .layout_runs()
            .flat_map(|line| line.glyphs.iter().map(|glyph| glyph.font_id))
            .collect()
    }

    const SAMPLE: &str = "Settings › Colours and type  ↑↓ MOVE · ENTER CHOOSE";

    #[test]
    fn every_role_asks_for_a_weight_and_style_the_bundled_face_has() {
        // A role at a weight the bundled face lacks is drawn either in another family (with fonts
        // installed) or at a weight it does not have (without) — the type scale saying one thing
        // and the screen another. Labels were 500 and the wordmark 600, and both were the first.
        let (db, bundled) = bundled_only();
        let weights = bundled_weights(&db, &bundled);
        assert!(!weights.is_empty(), "the bundled face has a weight");
        for role in Role::ALL {
            // Exhaustive, so a new role cannot be added without being seen here and put in `ALL`.
            match role {
                Role::Display
                | Role::Prose
                | Role::Code
                | Role::Comment
                | Role::LineNumber
                | Role::Value
                | Role::Label
                | Role::Ui
                | Role::Brand => {}
            }
            let spec = Typography::default().spec(role);
            assert!(
                weights.contains(&spec.weight),
                "{role:?} asks for weight {} but Fira Code is bundled at {weights:?}",
                spec.weight
            );
            // The face has no italic. One would still be drawn in Fira Code, slanted (see the test
            // below), but no role is meant to be, and one that became so should be a decision.
            assert_eq!(spec.style, Style::Roman, "{role:?} is italic");
        }
    }

    #[test]
    fn the_decoys_would_take_a_weight_the_bundled_face_lacks() {
        // Proves the next test can fail: asked for 600 directly, with the stand-ins installed,
        // cosmic-text leaves Fira Code — which is the defect, reproduced without a GPU or a system
        // font. If this ever stops holding, the guard below is checking nothing.
        let (mut font_system, bundled) = with_decoys();
        let raw = Attrs::new()
            .family(CtFamily::Name("Fira Code"))
            .weight(CtWeight(600));
        let faces = faces_used(&mut font_system, SAMPLE, &raw);
        assert!(
            faces.iter().any(|id| !bundled.contains(id)),
            "a 600 request stayed in Fira Code even with fallbacks installed"
        );
    }

    #[test]
    fn every_weight_and_style_is_shaped_in_the_bundled_face() {
        // The shaping path, not the type scale: whatever weight a caller writes into a spec — the
        // settings page once set 600 by hand on its headings and row names — and either style,
        // every glyph comes from Fira Code while a family that has that exact weight is installed.
        let (mut font_system, bundled) = with_decoys();
        let weights = bundled_weights(font_system.db(), &bundled);
        let base = Typography::default().spec(Role::Ui);
        for weight in (100..=900).step_by(50) {
            for style in [Style::Roman, Style::Italic] {
                let spec = dacode_theme::FontSpec {
                    weight,
                    style,
                    ..base
                };
                let attrs = attrs(CtFamily::Name("Fira Code"), &spec, &weights);
                let faces = faces_used(&mut font_system, SAMPLE, &attrs);
                assert!(!faces.is_empty());
                assert!(
                    faces.iter().all(|id| bundled.contains(id)),
                    "weight {weight} {style:?} left Fira Code"
                );
            }
        }
    }

    #[test]
    fn a_weight_snaps_to_the_nearest_bundled_one() {
        assert_eq!(shaped_weight(600, &[400]), 400);
        assert_eq!(shaped_weight(600, &[300, 400, 500, 700]), 500);
        // A tie goes to the lighter: a heading a little light reads better than one too heavy.
        assert_eq!(shaped_weight(600, &[500, 700]), 500);
        assert_eq!(shaped_weight(400, &[300, 400, 500]), 400);
    }

    #[test]
    fn coverage_is_read_from_the_bundled_face() {
        // The coverage guard over the interface's strings is only as good as this answer.
        assert!(fira_code_draws('a'));
        assert!(fira_code_draws('›'));
        assert!(fira_code_draws('→'));
        assert!(!fira_code_draws('漢'));
        assert!(!fira_code_draws('😀'));
    }
}

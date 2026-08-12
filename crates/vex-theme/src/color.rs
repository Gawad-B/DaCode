//! Color, in OKLCH.
//!
//! The design specifies every color as `oklch(L C H)`, and that is not incidental: the mode
//! accents are *one lightness and one chroma with four hues*, which is what makes the four
//! modes read as one instrument changing state rather than as four themes. Storing them as
//! hex would throw away exactly the relationship the identity depends on, so colors are kept
//! perceptual and converted to linear RGB at the point where the GPU needs them.

/// A color in the OKLCH perceptual space: lightness `0..1`, chroma `0..~0.4`, hue in degrees.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub alpha: f32,
}

impl Oklch {
    pub const fn new(l: f32, c: f32, h: f32) -> Self {
        Self {
            l,
            c,
            h,
            alpha: 1.0,
        }
    }

    pub const fn with_alpha(self, alpha: f32) -> Self {
        Self { alpha, ..self }
    }

    /// Same color, different hue — how every mode accent is derived.
    pub const fn at_hue(self, h: f32) -> Self {
        Self { h, ..self }
    }

    /// Same hue and chroma at a different lightness. Used when an imported theme leaves text
    /// illegible against its own background: the author's hue is kept, the lightness is fixed.
    pub const fn with_lightness(self, l: f32) -> Self {
        Self { l, ..self }
    }

    /// Premultiplied-alpha-free linear RGB, which is what a wgpu render pipeline wants.
    pub fn to_linear(self) -> [f32; 4] {
        let (l, c, h) = (self.l, self.c, self.h.to_radians());
        let (a, b) = (c * h.cos(), c * h.sin());

        // OKLab -> LMS' -> LMS
        let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
        let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
        let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
        let (lc, mc, sc) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

        // LMS -> linear sRGB
        let r = 4.076_741_7 * lc - 3.307_711_6 * mc + 0.230_969_94 * sc;
        let g = -1.268_438 * lc + 2.609_757_4 * mc - 0.341_319_4 * sc;
        let bl = -0.004_196_086_3 * lc - 0.703_418_6 * mc + 1.707_614_7 * sc;

        [
            r.clamp(0.0, 1.0),
            g.clamp(0.0, 1.0),
            bl.clamp(0.0, 1.0),
            self.alpha,
        ]
    }

    /// 8-bit sRGB, for logging, config round-tripping and anything that must speak hex.
    pub fn to_srgb8(self) -> [u8; 4] {
        let linear = self.to_linear();
        let encode = |v: f32| {
            let s = if v <= 0.003_130_8 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (s.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        [
            encode(linear[0]),
            encode(linear[1]),
            encode(linear[2]),
            (self.alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    /// Converts 8-bit sRGB into OKLCH — the inverse of [`Self::to_srgb8`].
    ///
    /// Needed to import themes authored in hex. Going through OKLCH rather than storing the hex
    /// means an imported theme participates in the same relationships as a native one: its accents
    /// can be re-lightened for a light background, and its chroma can be compared against the
    /// palette's family.
    pub fn from_srgb8(r: u8, g: u8, b: u8) -> Self {
        let decode = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (decode(r), decode(g), decode(b));

        // Linear sRGB -> LMS
        let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
        let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
        let s = 0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_5 * b;

        let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());

        // LMS -> OKLab
        let lightness = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
        let a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
        let b = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;

        let chroma = (a * a + b * b).sqrt();
        let hue = b.atan2(a).to_degrees();

        Self {
            l: lightness,
            c: chroma,
            h: if hue < 0.0 { hue + 360.0 } else { hue },
            alpha: 1.0,
        }
    }

    /// Parses either notation a config file may use: `oklch(L C H)` or `#rrggbb`.
    ///
    /// Both, because they serve different people. `oklch()` is the form the importer writes and the
    /// form a theme stays *adjustable* in — nudging lightness without shifting hue is a subtraction
    /// in OKLCH and guesswork in hex. Hex is what everyone already has pasted in their notes.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if let Some(inner) = input
            .strip_prefix("oklch(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let mut parts = inner.split_whitespace();
            let l = parts.next()?.parse().ok()?;
            let c = parts.next()?.parse().ok()?;
            let h = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            return Some(Self::new(l, c, h));
        }
        Self::from_hex(input)
    }

    /// Parses `#rrggbb`, `#rrggbbaa` or `#rgb`, as themes are written.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim().trim_start_matches('#');
        let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();

        let (r, g, b, a) = match hex.len() {
            3 => {
                let mut chars = hex.chars();
                (
                    expand(chars.next()?)?,
                    expand(chars.next()?)?,
                    expand(chars.next()?)?,
                    255,
                )
            }
            6 | 8 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                if hex.len() == 8 {
                    u8::from_str_radix(&hex[6..8], 16).ok()?
                } else {
                    255
                },
            ),
            _ => return None,
        };

        Some(Self::from_srgb8(r, g, b).with_alpha(a as f32 / 255.0))
    }

    pub fn to_hex(self) -> String {
        let [r, g, b, _] = self.to_srgb8();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// Composites `self` over `under` — used for the mode tints, which are specified as low
    /// alpha over the canvas and must be resolved before they reach the vertex buffer.
    pub fn over(self, under: Oklch) -> [f32; 4] {
        let top = self.to_linear();
        let bottom = under.to_linear();
        let a = self.alpha.clamp(0.0, 1.0);
        [
            top[0] * a + bottom[0] * (1.0 - a),
            top[1] * a + bottom[1] * (1.0 - a),
            top[2] * a + bottom[2] * (1.0 - a),
            1.0,
        ]
    }
}

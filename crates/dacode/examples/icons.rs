//! Regenerates the application icons from the chevron mark.
//!
//! Run with `cargo run -p dacode --example icons`. It rewrites `packaging/linux/icons` and
//! `crates/dacode/assets/icons`, so the icon that ships is the icon in the repository rather than
//! whatever was exported by hand once.
//!
//! # Why the icon has a background
//!
//! It did not, and that is the bug this exists for. The mark is a near-white chevron, shipped on
//! transparency because "desktop environments composite app icons onto panels and docks that are
//! dark far more often than light" — a reasonable bet, and wrong for the first person who installed
//! the rpm. Their application menu is light, and a near-white mark on transparency there is not a
//! faint icon, it is *no icon*: a blank tile with the word DaCode under it.
//!
//! An icon that depends on what is behind it has a coin flip in it. This one carries the editor's
//! own background, so it reads on any panel, any theme, any platform — which is also what every
//! other application in that menu does.
//!
//! Drawn here rather than exported from a design tool because the alternative is a binary asset
//! nobody can regenerate: the shipped PNGs and the SVG have to agree, and eight sizes exported by
//! hand drift the first time one of them is touched.

use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// The editor's own background, and the paper the mark is drawn in.
const BACKGROUND: [u8; 3] = [0x1c, 0x1a, 0x2b];
const INK: [u8; 3] = [0xe8, 0xe4, 0xdc];

/// How rounded the tile is, as a fraction of its side. Matches the platform convention closely
/// enough to sit beside other icons without looking like a sticker.
const RADIUS: f32 = 0.22;

/// Supersampling for the edges. The shapes are diagonal strokes and a rounded rectangle, both of
/// which alias badly at 16px without it.
const SAMPLES: u32 = 4;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root is two above this crate")
        .to_path_buf();

    let hicolor = root.join("packaging/linux/icons/hicolor");
    for size in [16u32, 24, 32, 48, 64, 128, 256, 512] {
        let pixels = render(size);
        write_png(
            &hicolor.join(format!("{size}x{size}/apps/dacode.png")),
            &pixels,
            size,
        );
    }

    // The window icon, embedded in the binary. The same art, so an installed editor and one run out
    // of a build directory cannot look different.
    write_png(
        &root.join("crates/dacode/assets/icons/dacode-256.png"),
        &render(256),
        256,
    );

    write_svg(&hicolor.join("scalable/apps/dacode.svg"));
    println!("dacode: wrote the icon set under {}", hicolor.display());
}

/// One icon, as RGBA rows.
///
/// Everything is in unit coordinates so the geometry is written once and the sizes are only a
/// sampling rate — an icon set whose 16px and 512px versions were drawn separately would not be the
/// same icon.
fn render(size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let side = size as f32;

    for y in 0..size {
        for x in 0..size {
            let (mut tile, mut upper, mut lower) = (0.0f32, 0.0f32, 0.0f32);

            // Supersampled coverage: the fraction of this pixel each shape covers.
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let step = 1.0 / (SAMPLES as f32 + 1.0);
                    let u = (x as f32 + (sx as f32 + 1.0) * step) / side;
                    let v = (y as f32 + (sy as f32 + 1.0) * step) / side;
                    if inside_tile(u, v) {
                        tile += 1.0;
                        // Clipped to the tile, so a stroke cannot bleed past a rounded corner.
                        if on_chevron(u, v, 0.235) {
                            upper += 1.0;
                        }
                        if on_chevron(u, v, 0.465) {
                            lower += 1.0;
                        }
                    }
                }
            }

            let total = (SAMPLES * SAMPLES) as f32;
            let (tile, upper, lower) = (tile / total, upper / total, lower / total);

            // The lower chevron is the same mark at a third of the weight, as in the wordmark.
            let ink = (upper + lower * 0.34).min(1.0);
            let index = ((y * size + x) * 4) as usize;
            for channel in 0..3 {
                let base = BACKGROUND[channel] as f32;
                let mixed = base + (INK[channel] as f32 - base) * ink;
                pixels[index + channel] = mixed.round().clamp(0.0, 255.0) as u8;
            }
            pixels[index + 3] = (tile * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    pixels
}

/// Whether a point is inside the rounded tile.
fn inside_tile(u: f32, v: f32) -> bool {
    let r = RADIUS;
    // The straight edges cover everything but the four corner squares.
    let near_x = u < r || u > 1.0 - r;
    let near_y = v < r || v > 1.0 - r;
    if !(near_x && near_y) {
        return (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v);
    }
    let cx = if u < r { r } else { 1.0 - r };
    let cy = if v < r { r } else { 1.0 - r };
    ((u - cx).powi(2) + (v - cy).powi(2)).sqrt() <= r
}

/// Whether a point is on a chevron whose apex sits `top` down the tile.
///
/// Two strokes meeting at a point, measured as distance to each segment — which keeps the join
/// mitred without any path arithmetic.
fn on_chevron(u: f32, v: f32, top: f32) -> bool {
    const HALF_WIDTH: f32 = 0.26;
    const DROP: f32 = 0.30;
    const WEIGHT: f32 = 0.088;

    let apex = (0.5, top + DROP);
    let left = (0.5 - HALF_WIDTH, top);
    let right = (0.5 + HALF_WIDTH, top);

    distance_to_segment((u, v), left, apex) <= WEIGHT / 2.0
        || distance_to_segment((u, v), right, apex) <= WEIGHT / 2.0
}

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let closest = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - closest.0).powi(2) + (p.1 - closest.1).powi(2)).sqrt()
}

/// The scalable version, written from the same numbers so it cannot drift from the rasters.
fn write_svg(path: &Path) {
    const HALF_WIDTH: f32 = 0.26;
    const DROP: f32 = 0.30;
    const WEIGHT: f32 = 0.088;
    let pct = |value: f32| value * 100.0;

    let chevron = |top: f32, opacity: &str| {
        format!(
            r#"<path d="M{} {} L50 {} L{} {}" opacity="{opacity}"/>"#,
            pct(0.5 - HALF_WIDTH),
            pct(top),
            pct(top + DROP),
            pct(0.5 + HALF_WIDTH),
            pct(top),
        )
    };

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="512" height="512">
<rect width="100" height="100" rx="{radius}" fill="#{bg:02x}{bg1:02x}{bg2:02x}"/>
<g fill="none" stroke="#{ink:02x}{ink1:02x}{ink2:02x}" stroke-width="{weight}" stroke-linecap="butt" stroke-linejoin="miter">
{upper}
{lower}
</g>
</svg>
"##,
        radius = pct(RADIUS),
        bg = BACKGROUND[0],
        bg1 = BACKGROUND[1],
        bg2 = BACKGROUND[2],
        ink = INK[0],
        ink1 = INK[1],
        ink2 = INK[2],
        weight = pct(WEIGHT),
        upper = chevron(0.235, "1"),
        lower = chevron(0.465, "0.34"),
    );

    std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the icon directory");
    std::fs::write(path, svg).expect("writing the svg");
}

fn write_png(path: &PathBuf, pixels: &[u8], size: u32) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the icon directory");
    let file = std::fs::File::create(path).expect("creating the icon");
    let mut encoder = png::Encoder::new(BufWriter::new(file), size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("writing the header")
        .write_image_data(pixels)
        .expect("writing the pixels");
}

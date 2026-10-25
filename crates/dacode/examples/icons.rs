//! Regenerates every icon and logo image from the mark's two masks.
//!
//! Run with `cargo run -p dacode --example icons`. It rewrites `packaging/linux/icons`,
//! `packaging/windows/dacode.ico`, `crates/dacode/assets/icons` and `logo/dacode`, so the icon that
//! ships is the icon in the repository rather than whatever was exported by hand once.
//!
//! # Where the art comes from
//!
//! `crates/dacode-render/assets/mark.png` and `mark-small.png` are the logo — direction B of the
//! logo sheet, "bold geometric". Each is a pair of coverage masks rather than a picture: red is how
//! much of a pixel the letter covers, green how much the amber block does. They were separated from
//! the sheet once, against its own background and its two ink colours, and are the source of truth
//! from here on: the editor draws its title-bar mark from the same files, so the icon in the dock and
//! the mark in the window cannot disagree. `mark-small.png` is the sheet's own small-size variant,
//! where the white and amber blocks become one block below 64 pixels.
//!
//! # Why the icon has a background
//!
//! The mark is a near-white letter, and a near-white letter on transparency in a light application
//! menu is not a faint icon, it is *no icon* — which is what the first person to install the rpm
//! saw, with the old mark. The icon carries the logo sheet's own dark tile, so it reads on any panel,
//! any theme, any platform, as every other application in that menu does.

use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// The tile, the letter and the block, from the logo sheet.
const TILE: [u8; 3] = [0x10, 0x16, 0x22];
const LETTER: [u8; 3] = [0xe8, 0xeb, 0xf0];
const BLOCK: [u8; 3] = [0xf1, 0xb7, 0x49];
/// The letter's colour on a light surface, from the sheet's light card.
const LETTER_ON_LIGHT: [u8; 3] = [0x14, 0x17, 0x20];

/// How rounded the tile is, as a fraction of its side.
const RADIUS: f32 = 0.22;
/// How much of the tile the mark spans.
const MARK: f32 = 0.74;
/// Below this the sheet's small variant is used.
const SMALL_BELOW: u32 = 64;

const MASK_SIDE: u32 = 512;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root is two above this crate")
        .to_path_buf();
    let assets = root.join("crates/dacode-render/assets");
    let full = load_mask(&assets.join("mark.png"));
    let small = load_mask(&assets.join("mark-small.png"));
    let mask_for = |size: u32| if size < SMALL_BELOW { &small } else { &full };

    let hicolor = root.join("packaging/linux/icons/hicolor");
    for size in [16u32, 24, 32, 48, 64, 128, 256, 512] {
        write_png(
            &hicolor.join(format!("{size}x{size}/apps/dacode.png")),
            &tile(mask_for(size), size),
            size,
        );
    }

    // The window icon, embedded in the binary. The same art, so an installed editor and one run out
    // of a build directory cannot look different.
    write_png(
        &root.join("crates/dacode/assets/icons/dacode-256.png"),
        &tile(&full, 256),
        256,
    );

    write_svg(
        &hicolor.join("scalable/apps/dacode.svg"),
        &encode_png(&tile(&full, 512), 512),
    );

    let ico_sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let images: Vec<(u32, Vec<u8>)> = ico_sizes
        .iter()
        .map(|&size| (size, encode_png(&tile(mask_for(size), size), size)))
        .collect();
    write_ico(&root.join("packaging/windows/dacode.ico"), &images);

    // The bare mark for documents: the README picks one by the reader's colour scheme.
    let logo = root.join("logo/dacode");
    write_png(
        &logo.join("mark-on-dark-128.png"),
        &bare(&full, 128, LETTER),
        128,
    );
    write_png(
        &logo.join("mark-on-light-128.png"),
        &bare(&full, 128, LETTER_ON_LIGHT),
        128,
    );

    println!("dacode: wrote the icon set from {}", assets.display());
}

/// A mask as (letter, block) coverage pairs, row by row.
fn load_mask(path: &Path) -> Vec<(f32, f32)> {
    let file = std::fs::File::open(path).expect("the mark's masks are in the repository");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut pixels).expect("decodes");
    assert_eq!(
        (info.width, info.height),
        (MASK_SIDE, MASK_SIDE),
        "the masks are {MASK_SIDE} square"
    );
    let channels = info.color_type.samples();
    pixels[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|pixel| (pixel[0] as f32 / 255.0, pixel[1] as f32 / 255.0))
        .collect()
}

/// The mask's average coverage over a square of the source, for resampling it down.
///
/// An area average rather than a point sample: at 16 pixels each output pixel covers a 32-pixel
/// square of the mask, and sampling one point of it is how small icons come out jagged.
fn coverage(mask: &[(f32, f32)], x0: f32, y0: f32, span: f32) -> (f32, f32) {
    let (mut letter, mut block, mut count) = (0.0, 0.0, 0.0);
    let from_x = x0.floor().max(0.0) as u32;
    let from_y = y0.floor().max(0.0) as u32;
    let to_x = ((x0 + span).ceil() as u32).min(MASK_SIDE);
    let to_y = ((y0 + span).ceil() as u32).min(MASK_SIDE);
    for y in from_y..to_y {
        for x in from_x..to_x {
            let (l, b) = mask[(y * MASK_SIDE + x) as usize];
            letter += l;
            block += b;
            count += 1.0;
        }
    }
    if count == 0.0 {
        (0.0, 0.0)
    } else {
        (letter / count, block / count)
    }
}

/// The mark on its tile, as RGBA rows.
fn tile(mask: &[(f32, f32)], size: u32) -> Vec<u8> {
    let side = size as f32;
    let mark = side * MARK;
    let offset = (side - mark) / 2.0;
    let span = MASK_SIDE as f32 / mark;

    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (mut letter, mut block) = (0.0, 0.0);
            let (mx, my) = (x as f32 - offset, y as f32 - offset);
            if mx >= 0.0 && my >= 0.0 && mx < mark && my < mark {
                (letter, block) = coverage(mask, mx * span, my * span, span);
            }
            let index = ((y * size + x) * 4) as usize;
            for channel in 0..3 {
                // The block on the tile, then the letter over both — as the artwork overlaps them.
                let under =
                    TILE[channel] as f32 + (BLOCK[channel] as f32 - TILE[channel] as f32) * block;
                let value = under + (LETTER[channel] as f32 - under) * letter;
                pixels[index + channel] = value.round().clamp(0.0, 255.0) as u8;
            }
            pixels[index + 3] = (tile_coverage(x, y, size) * 255.0).round() as u8;
        }
    }
    pixels
}

/// The mark alone on transparency, its letter in `ink`.
fn bare(mask: &[(f32, f32)], size: u32, ink: [u8; 3]) -> Vec<u8> {
    let span = MASK_SIDE as f32 / size as f32;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (letter, block) = coverage(mask, x as f32 * span, y as f32 * span, span);
            let alpha = letter + block * (1.0 - letter);
            let index = ((y * size + x) * 4) as usize;
            if alpha > 0.0 {
                for channel in 0..3 {
                    let value = (ink[channel] as f32 * letter
                        + BLOCK[channel] as f32 * block * (1.0 - letter))
                        / alpha;
                    pixels[index + channel] = value.round().clamp(0.0, 255.0) as u8;
                }
            }
            pixels[index + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    pixels
}

/// How much of a pixel the rounded tile covers, supersampled four by four.
fn tile_coverage(x: u32, y: u32, size: u32) -> f32 {
    let side = size as f32;
    let mut inside = 0.0;
    for sy in 0..4 {
        for sx in 0..4 {
            let u = (x as f32 + (sx as f32 + 0.5) / 4.0) / side;
            let v = (y as f32 + (sy as f32 + 0.5) / 4.0) / side;
            let cx = u.clamp(RADIUS, 1.0 - RADIUS);
            let cy = v.clamp(RADIUS, 1.0 - RADIUS);
            if (u - cx).hypot(v - cy) <= RADIUS {
                inside += 1.0;
            }
        }
    }
    inside / 16.0
}

/// The scalable icon. The artwork is a raster at heart — it came from one — so the SVG carries the
/// 512 pixel tile rather than pretending to be vectors it was never drawn as.
fn write_svg(path: &Path, png_bytes: &[u8]) {
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
<image width="512" height="512" href="data:image/png;base64,{}"/>
</svg>
"#,
        base64(png_bytes)
    );
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the icon directory");
    std::fs::write(path, svg).expect("writing the svg");
}

/// A Windows icon with a PNG per size, which every Windows since Vista reads.
fn write_ico(path: &Path, images: &[(u32, Vec<u8>)]) {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        // A side of 256 is written as 0: the field is one byte.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    std::fs::write(path, out).expect("writing the ico");
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn encode_png(pixels: &[u8], size: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("writing the header")
            .write_image_data(pixels)
            .expect("writing the pixels");
    }
    bytes
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

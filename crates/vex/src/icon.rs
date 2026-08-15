//! The window icon.
//!
//! Embedded rather than loaded from disk, for the same reason the fonts are: an installed editor and
//! one run out of a build directory must look identical, and an icon that silently fails to load is
//! the kind of defect nobody files a bug about.
//!
//! Decoded with the `png` crate, which is already here for the offscreen capture. Pulling in `image`
//! — the obvious choice, and what the handoff suggests — would add a decoder stack for a dozen
//! formats to read one file whose format we chose ourselves.
//!
//! The icon carries its own background. It used to be the bare "paper" mark on transparency, on the
//! reasoning that "desktop environments composite app icons onto panels and docks that are dark far
//! more often than light" — a bet, and one that came back as a bug report from the first person to
//! install the rpm. Their application menu is light, and a near-white mark on transparency there is
//! not a faint icon: it is a blank tile with the word vex under it.
//!
//! An icon whose legibility depends on what is behind it has a coin flip in it. This one is drawn on
//! the editor's own background, like nearly every other application in that menu, so there is
//! nothing left to guess. `cargo run -p vex --example icons` regenerates it and the whole Linux set
//! from one definition.

use winit::window::Icon;

const ICON_PNG: &[u8] = include_bytes!("../assets/icons/vex-256.png");

/// Decodes the icon. `None` rather than an error: a window without an icon is a cosmetic loss and
/// must never be a reason the editor fails to start.
pub fn window_icon() -> Option<Icon> {
    match decode(ICON_PNG) {
        Ok((rgba, width, height)) => match Icon::from_rgba(rgba, width, height) {
            Ok(icon) => Some(icon),
            Err(err) => {
                eprintln!("vex: window icon rejected: {err}");
                None
            }
        },
        Err(err) => {
            eprintln!("vex: could not decode the window icon: {err}");
            None
        }
    }
}

/// Decodes a PNG to straight RGBA8.
///
/// The mark's PNGs are RGBA already, but this handles the grey and palette cases too, because an
/// icon that is silently rejected for its color type is worse than a few lines of expansion.
fn decode(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), png::DecodingError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());

    let pixels = (info.width * info.height) as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        // `read_info` expands palettes for us, so this is unreachable in practice.
        png::ColorType::Indexed => buffer,
    };

    debug_assert_eq!(rgba.len(), pixels * 4, "icon did not expand to RGBA");
    Ok((rgba, info.width, info.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_decodes_to_square_rgba() {
        let (rgba, width, height) = decode(ICON_PNG).expect("the shipped icon decodes");

        assert_eq!(
            width, 256,
            "the embedded size is what the handoff specifies"
        );
        assert_eq!(height, 256);
        assert_eq!(rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn the_icon_is_a_rounded_tile_rather_than_a_square_or_a_bare_mark() {
        // Both halves matter. A bare mark on transparency is the defect this replaced — invisible
        // on a light menu. A *square* tile is the other failure: it reads as a sticker beside the
        // rounded icons every desktop draws, so the corners have to stay empty.
        let (rgba, width, _) = decode(ICON_PNG).unwrap();
        let alpha_at = |x: usize, y: usize| rgba[(y * width as usize + x) * 4 + 3];

        assert_eq!(alpha_at(2, 2), 0, "top-left corner is rounded away");
        assert_eq!(alpha_at(253, 2), 0, "top-right corner is rounded away");
        assert_eq!(alpha_at(2, 253), 0, "bottom-left corner is rounded away");

        assert_eq!(
            alpha_at(128, 4),
            255,
            "the tile itself is solid at its edge"
        );
        assert_eq!(alpha_at(4, 128), 255);
        // The apex of the primary chevron is around 50%,52% of the box.
        assert!(alpha_at(128, 130) > 200, "the mark is opaque at its apex");
    }

    #[test]
    fn the_mark_stands_out_against_the_ground_it_is_drawn_on() {
        // This used to assert the mark was *light*, on the reasoning that docks are usually dark —
        // and that is the assertion that would have held the invisible icon in place. What matters
        // is not which way round the two are, it is that they differ: an icon legible on one
        // desktop and blank on another is the bug, whichever desktop drew the short straw.
        let (rgba, width, _) = decode(ICON_PNG).unwrap();
        let sample = |x: usize, y: usize| {
            let index = (y * width as usize + x) * 4;
            (
                rgba[index],
                rgba[index + 1],
                rgba[index + 2],
                rgba[index + 3],
            )
        };

        let (mr, mg, mb, ma) = sample(128, 130);
        let (gr, gg, gb, ga) = sample(128, 232);
        assert_eq!(ma, 255, "the mark is opaque");
        assert_eq!(ga, 255, "so is the ground under it");

        // Rough luminance is enough: this is asking "can you see it", not matching a colour.
        let luma = |r: u8, g: u8, b: u8| 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        let separation = (luma(mr, mg, mb) - luma(gr, gg, gb)).abs();
        assert!(
            separation > 90.0,
            "the mark rgb({mr},{mg},{mb}) barely separates from the ground rgb({gr},{gg},{gb})"
        );
    }
}

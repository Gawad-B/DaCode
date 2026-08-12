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
//! The light ("paper") variant is the shipped icon: desktop environments composite app icons onto
//! panels and docks that are dark far more often than light.

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
    fn the_icon_is_transparent_at_its_corners_and_marked_in_its_middle() {
        // The mark sits on transparency with no tile, so the corners must be empty — an icon that
        // arrived with a background would have come from the wrong export.
        let (rgba, width, _) = decode(ICON_PNG).unwrap();
        let alpha_at = |x: usize, y: usize| rgba[(y * width as usize + x) * 4 + 3];

        assert_eq!(alpha_at(2, 2), 0, "top-left corner is transparent");
        assert_eq!(alpha_at(253, 2), 0, "top-right corner is transparent");

        // The apex of the primary chevron is around 50%,52% of the box.
        assert!(
            alpha_at(128, 130) > 200,
            "the mark itself is opaque where the apex is"
        );
    }

    #[test]
    fn the_icon_is_the_paper_variant() {
        // Light ink, because docks are usually dark. If someone swaps in the `ink` export by mistake
        // the icon vanishes into most panels, which is exactly the kind of thing nobody notices in
        // review.
        let (rgba, width, _) = decode(ICON_PNG).unwrap();
        let index = (130 * width as usize + 128) * 4;
        let (r, g, b) = (rgba[index], rgba[index + 1], rgba[index + 2]);

        assert!(
            r > 180 && g > 180 && b > 170,
            "expected the light mark, got rgb({r}, {g}, {b})"
        );
    }
}

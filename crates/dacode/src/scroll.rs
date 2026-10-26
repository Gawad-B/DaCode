//! Scrolling by pixels.
//!
//! A touchpad reports scrolling as a stream of small pixel deltas, a few pixels each. The wheel
//! handler used to turn every delta into whole lines on arrival — divide by forty, truncate — so a
//! three-pixel delta became zero lines, every one of them, and a touchpad scrolled nothing at all.
//! The repair is to keep what has not yet added up to a row: [`ScrollInput`] holds the remainder,
//! whole rows move the [`ScrollAnchor`], and the part of a row left over is drawn as a sub-row
//! offset of the code area, so the text follows the fingers one-to-one.
//!
//! The anchor stays the source of truth for which line is at the top. The remainder only ever says
//! how far that top row has moved up out of view, and it is kept in `[0, row_height)` by
//! [`by_pixels`], so a negative remainder never opens a gap above the first row.

use dacode_display::{DisplayMap, ScrollAnchor};
use dacode_text::Text;

/// Pixels of scrolling that have not yet added up to a whole row.
///
/// Positive is towards the end of the document: the content moves up.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollInput {
    pending_px: f32,
}

impl ScrollInput {
    /// Adds `px` pixels and returns how many whole rows that makes, keeping the rest.
    ///
    /// Rounded toward zero, so the remainder carries the sign of the motion: scrolling back spends
    /// what was left over before it moves a row the other way.
    pub fn add_pixels(&mut self, px: f32, row_height: f32) -> isize {
        // A NaN would poison the remainder for good, and every later delta with it.
        if !px.is_finite() || row_height <= 0.0 || !row_height.is_finite() {
            return 0;
        }
        self.pending_px += px;
        let ratio = self.pending_px / row_height;
        // Within a hair of a whole row counts as one. Twenty deltas of a twentieth of a row sum in
        // `f32` to a fraction under one, and truncation would then hold the row back for no reason
        // a person could see.
        let nearest = ratio.round();
        let rows = if (ratio - nearest).abs() < 1e-4 {
            nearest
        } else {
            ratio.trunc()
        };
        self.pending_px -= rows * row_height;
        rows as isize
    }

    /// What has not yet made a whole row, in pixels.
    pub fn remainder(&self) -> f32 {
        self.pending_px
    }

    /// Forgets the remainder. For anything that moves the view other than by pixels: the remainder
    /// was measured from the old top row and means nothing against a new one.
    pub fn clear(&mut self) {
        self.pending_px = 0.0;
    }
}

/// Scrolls `anchor` by `px` pixels, keeping the part of a row left over in `input`.
///
/// Afterwards the remainder is in `[0, row_height)`: a negative one is drawn from the row above, by
/// stepping the anchor back a row and adding a row's height. At the first line there is no row
/// above, and at the last there is nothing further to show, so a remainder pushing past either end
/// is dropped — the same clamp whole-row scrolling has always had.
///
/// The cost is the steps the delta makes plus at most two more, each a walk of one line's wrap,
/// whatever the document's size.
pub fn by_pixels(
    display: &DisplayMap,
    text: &Text,
    anchor: ScrollAnchor,
    input: &mut ScrollInput,
    px: f32,
    row_height: f32,
) -> ScrollAnchor {
    let rows = input.add_pixels(px, row_height);
    let mut anchor = if rows == 0 {
        anchor
    } else {
        display.step(text, anchor, rows)
    };

    if input.remainder() < 0.0 {
        let back = display.step(text, anchor, -1);
        // Nothing above, or a remainder so close to zero that adding a row would round to a whole
        // row hidden: either way there is nothing to draw from the row above.
        if back == anchor || input.remainder() > -0.01 {
            input.clear();
        } else {
            anchor = back;
            input.pending_px += row_height;
        }
    }
    if input.remainder() > 0.0 && display.step(text, anchor, 1) == anchor {
        input.clear();
    }
    anchor
}

/// Settles a remainder on the nearest whole row, for the end of a wheel glide.
///
/// A glide's target is a whole number of rows away from where it started, so where it stops is a
/// row boundary — except for the rounding of a few hundred frame-sized deltas, which can leave the
/// view a hair short. Left alone that hair would be a sliver of the row above showing under the
/// tab bar until the next scroll.
pub fn land_on_row(
    display: &DisplayMap,
    text: &Text,
    anchor: ScrollAnchor,
    input: &mut ScrollInput,
    row_height: f32,
) -> ScrollAnchor {
    let remainder = input.remainder();
    input.clear();
    if remainder >= row_height / 2.0 {
        display.step(text, anchor, 1)
    } else if remainder <= -row_height / 2.0 {
        display.step(text, anchor, -1)
    } else {
        anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: f32 = 26.0;

    fn lines(count: usize) -> Text {
        Text::from_str(
            &(0..count)
                .map(|i| format!("line {i}\n"))
                .collect::<String>(),
        )
    }

    #[test]
    fn many_small_touchpad_deltas_add_up_to_rows() {
        // The bug: each 3-pixel delta was divided by 40 and truncated to 0, so a touchpad scrolled nothing.
        let mut input = ScrollInput::default();
        let rows: isize = (0..40).map(|_| input.add_pixels(3.0, 26.0)).sum();
        assert_eq!(rows, 4, "120 px is four 26 px rows");
        assert!((input.remainder() - 16.0).abs() < 1e-3);
    }

    #[test]
    fn reversing_direction_spends_the_remainder_first() {
        let mut input = ScrollInput::default();
        input.add_pixels(20.0, 26.0);
        assert_eq!(
            input.add_pixels(-30.0, 26.0),
            0,
            "-10 px net is less than a row"
        );
        assert!((input.remainder() + 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_touchpad_stream_moves_the_anchor_and_keeps_the_rest_as_an_offset() {
        let text = lines(200);
        let display = DisplayMap::default();
        let mut input = ScrollInput::default();
        let mut anchor = ScrollAnchor::at_line(10);
        for _ in 0..40 {
            anchor = by_pixels(&display, &text, anchor, &mut input, 3.0, ROW);
        }
        assert_eq!(anchor.line, 14);
        assert!((input.remainder() - 16.0).abs() < 1e-3);
    }

    #[test]
    fn scrolling_back_part_of_a_row_draws_from_the_row_above() {
        // A negative remainder drawn as it is would pull the top row *down*, opening a gap of
        // background between it and the tab bar. One row up with the rest as a positive offset is
        // the same position with nothing missing.
        let text = lines(200);
        let display = DisplayMap::default();
        let mut input = ScrollInput::default();
        let anchor = by_pixels(
            &display,
            &text,
            ScrollAnchor::at_line(10),
            &mut input,
            -10.0,
            ROW,
        );
        assert_eq!(anchor.line, 9);
        assert!((input.remainder() - 16.0).abs() < 1e-3);
    }

    #[test]
    fn nothing_scrolls_above_the_first_line_or_past_the_last() {
        let text = lines(200);
        let display = DisplayMap::default();

        let mut input = ScrollInput::default();
        let top = by_pixels(
            &display,
            &text,
            ScrollAnchor::at_line(0),
            &mut input,
            -40.0,
            ROW,
        );
        assert_eq!(top.line, 0);
        assert_eq!(input.remainder(), 0.0, "no gap opens above line one");

        let last = text.last_line();
        let mut input = ScrollInput::default();
        let bottom = by_pixels(
            &display,
            &text,
            ScrollAnchor::at_line(last - 1),
            &mut input,
            ROW * 5.5,
            ROW,
        );
        assert_eq!(bottom.line, last);
        assert_eq!(input.remainder(), 0.0, "the last line does not slide away");
    }

    #[test]
    fn a_glide_a_hair_short_of_a_row_lands_on_it() {
        let text = lines(200);
        let display = DisplayMap::default();

        let mut input = ScrollInput::default();
        input.add_pixels(ROW - 0.01, ROW);
        let anchor = land_on_row(&display, &text, ScrollAnchor::at_line(10), &mut input, ROW);
        assert_eq!(anchor.line, 11);
        assert_eq!(input.remainder(), 0.0);

        let mut input = ScrollInput::default();
        input.add_pixels(0.002, ROW);
        let anchor = land_on_row(&display, &text, ScrollAnchor::at_line(10), &mut input, ROW);
        assert_eq!(anchor.line, 10);
        assert_eq!(input.remainder(), 0.0);
    }
}

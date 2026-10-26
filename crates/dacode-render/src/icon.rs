//! The interface's icons, as geometry on a 24-unit grid.
//!
//! Drawn from segments, rings, arcs and discs rather than taken from a font: the bundled faces have
//! no icons, and every glyph this project has borrowed for one — a chevron, an arrow, a cross —
//! shipped at least once as a tofu box. Geometry cannot be missing from a font. It is also scaled,
//! antialiased and coloured by the same path as everything else, so an icon follows the theme and
//! the display's scale factor without a second asset per size.
//!
//! The grid and stroke weight follow the convention most icon sets share (24 units, a 2-unit stroke
//! with round ends), so these sit beside text the way people expect icons to.

/// One icon the chrome can draw.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Code in a rounded square: the explorer.
    Explorer,
    Search,
    Branch,
    Bug,
    Bulb,
    Gear,
    Bell,
    Home,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Close,
    Split,
    /// Filled.
    Play,
    Plus,
    Folder,
    File,
    ErrorCircle,
    Warning,
    Info,
    /// A filled dot: unsaved changes on a tab.
    Dot,
    Terminal,
    /// An open circle with its arrow turning back: put a setting back to its default.
    Reset,
}

/// A primitive in grid units, before it is placed and scaled.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Stroke {
    Line(f32, f32, f32, f32),
    Ring(f32, f32, f32),
    /// Centre, radius, start and end in degrees — 0 is +x, increasing clockwise on screen.
    Arc(f32, f32, f32, f32, f32),
    Disc(f32, f32, f32),
    Triangle(f32, f32, f32, f32, f32, f32),
}

use Stroke::{Arc, Disc, Line, Ring, Triangle};
use std::f32::consts::FRAC_1_SQRT_2;

/// A path through points, as segments. Round caps make the joins round too.
fn path(points: &[(f32, f32)]) -> Vec<Stroke> {
    points
        .windows(2)
        .map(|pair| Line(pair[0].0, pair[0].1, pair[1].0, pair[1].1))
        .collect()
}

/// A rounded rectangle's outline: four sides and four quarter arcs.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<Stroke> {
    let (right, bottom) = (x + w, y + h);
    vec![
        Line(x + r, y, right - r, y),
        Line(right, y + r, right, bottom - r),
        Line(right - r, bottom, x + r, bottom),
        Line(x, bottom - r, x, y + r),
        Arc(right - r, y + r, r, 270.0, 360.0),
        Arc(right - r, bottom - r, r, 0.0, 90.0),
        Arc(x + r, bottom - r, r, 90.0, 180.0),
        Arc(x + r, y + r, r, 180.0, 270.0),
    ]
}

impl Icon {
    pub const ALL: [Icon; 23] = [
        Icon::Explorer,
        Icon::Search,
        Icon::Branch,
        Icon::Bug,
        Icon::Bulb,
        Icon::Gear,
        Icon::Bell,
        Icon::Home,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::Close,
        Icon::Split,
        Icon::Play,
        Icon::Plus,
        Icon::Folder,
        Icon::File,
        Icon::ErrorCircle,
        Icon::Warning,
        Icon::Info,
        Icon::Dot,
        Icon::Terminal,
        Icon::Reset,
    ];

    pub fn strokes(self) -> Vec<Stroke> {
        match self {
            Icon::Explorer => {
                let mut strokes = rounded(3.0, 3.0, 18.0, 18.0, 4.0);
                strokes.extend(path(&[(10.0, 9.0), (7.5, 12.0), (10.0, 15.0)]));
                strokes.extend(path(&[(14.0, 9.0), (16.5, 12.0), (14.0, 15.0)]));
                strokes
            }
            Icon::Search => vec![Ring(11.0, 11.0, 7.0), Line(16.2, 16.2, 21.0, 21.0)],
            Icon::Branch => vec![
                Line(6.0, 3.0, 6.0, 15.0),
                Ring(18.0, 6.0, 3.0),
                Ring(6.0, 18.0, 3.0),
                Arc(9.0, 9.0, 9.0, 0.0, 90.0),
            ],
            Icon::Bug => {
                let mut strokes = rounded(8.0, 9.0, 8.0, 12.0, 4.0);
                strokes.extend([
                    Arc(12.0, 9.0, 3.0, 180.0, 360.0),
                    Line(12.0, 12.0, 12.0, 21.0),
                    Line(8.0, 13.0, 4.0, 12.0),
                    Line(8.0, 17.0, 4.0, 18.0),
                    Line(16.0, 13.0, 20.0, 12.0),
                    Line(16.0, 17.0, 20.0, 18.0),
                    Line(10.0, 6.5, 8.5, 3.5),
                    Line(14.0, 6.5, 15.5, 3.5),
                ]);
                strokes
            }
            Icon::Bulb => {
                let lower_left = (12.0 - 6.0 * FRAC_1_SQRT_2, 10.0 + 6.0 * FRAC_1_SQRT_2);
                let lower_right = (12.0 + 6.0 * FRAC_1_SQRT_2, 10.0 + 6.0 * FRAC_1_SQRT_2);
                vec![
                    Arc(12.0, 10.0, 6.0, 135.0, 405.0),
                    Line(lower_left.0, lower_left.1, 9.0, 17.0),
                    Line(lower_right.0, lower_right.1, 15.0, 17.0),
                    Line(9.0, 18.0, 15.0, 18.0),
                    Line(10.0, 21.0, 14.0, 21.0),
                ]
            }
            Icon::Gear => {
                let mut strokes = vec![Ring(12.0, 12.0, 3.0), Ring(12.0, 12.0, 7.0)];
                for step in 0..8 {
                    let angle = (step as f32 * 45.0).to_radians();
                    let (sin, cos) = angle.sin_cos();
                    strokes.push(Line(
                        12.0 + cos * 7.0,
                        12.0 + sin * 7.0,
                        12.0 + cos * 9.5,
                        12.0 + sin * 9.5,
                    ));
                }
                strokes
            }
            Icon::Bell => vec![
                Arc(12.0, 10.0, 6.0, 180.0, 360.0),
                Line(6.0, 10.0, 6.0, 15.0),
                Line(18.0, 10.0, 18.0, 15.0),
                Line(6.0, 15.0, 4.0, 17.0),
                Line(18.0, 15.0, 20.0, 17.0),
                Line(4.0, 17.0, 20.0, 17.0),
                Arc(12.0, 19.0, 2.0, 0.0, 180.0),
            ],
            Icon::Home => {
                let mut strokes = path(&[(3.0, 11.0), (12.0, 3.0), (21.0, 11.0)]);
                strokes.extend(path(&[(5.0, 9.5), (5.0, 21.0), (19.0, 21.0), (19.0, 9.5)]));
                strokes.extend(path(&[
                    (10.0, 21.0),
                    (10.0, 15.0),
                    (14.0, 15.0),
                    (14.0, 21.0),
                ]));
                strokes
            }
            Icon::ChevronLeft => path(&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)]),
            Icon::ChevronRight => path(&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]),
            Icon::ChevronDown => path(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]),
            Icon::Close => vec![Line(6.0, 6.0, 18.0, 18.0), Line(18.0, 6.0, 6.0, 18.0)],
            Icon::Split => {
                let mut strokes = rounded(3.0, 3.0, 18.0, 18.0, 3.0);
                strokes.push(Line(12.0, 3.0, 12.0, 21.0));
                strokes
            }
            Icon::Play => vec![Triangle(6.0, 4.0, 20.0, 12.0, 6.0, 20.0)],
            Icon::Plus => vec![Line(12.0, 5.0, 12.0, 19.0), Line(5.0, 12.0, 19.0, 12.0)],
            Icon::Folder => path(&[
                (3.0, 19.0),
                (3.0, 5.0),
                (9.0, 5.0),
                (11.0, 7.0),
                (21.0, 7.0),
                (21.0, 19.0),
                (3.0, 19.0),
            ]),
            Icon::File => {
                let mut strokes = path(&[
                    (6.0, 2.5),
                    (14.0, 2.5),
                    (19.0, 7.5),
                    (19.0, 21.5),
                    (6.0, 21.5),
                    (6.0, 2.5),
                ]);
                strokes.extend(path(&[(14.0, 2.5), (14.0, 7.5), (19.0, 7.5)]));
                strokes
            }
            Icon::ErrorCircle => vec![
                Ring(12.0, 12.0, 9.0),
                Line(9.0, 9.0, 15.0, 15.0),
                Line(15.0, 9.0, 9.0, 15.0),
            ],
            Icon::Warning => {
                let mut strokes = path(&[(12.0, 3.0), (22.0, 20.0), (2.0, 20.0), (12.0, 3.0)]);
                strokes.push(Line(12.0, 9.0, 12.0, 13.5));
                strokes.push(Disc(12.0, 16.8, 1.2));
                strokes
            }
            Icon::Info => vec![
                Ring(12.0, 12.0, 9.0),
                Line(12.0, 11.0, 12.0, 16.0),
                Disc(12.0, 7.8, 1.2),
            ],
            Icon::Dot => vec![Disc(12.0, 12.0, 4.5)],
            Icon::Terminal => {
                let mut strokes = path(&[(5.0, 8.0), (10.0, 12.0), (5.0, 16.0)]);
                strokes.push(Line(12.0, 17.0, 19.0, 17.0));
                strokes
            }
            // Three quarters of a circle from the top, clockwise round to the left, and the arrow at
            // the top end pointing back the way it came: the turn is anticlockwise, as "undo" is.
            Icon::Reset => {
                let mut strokes = vec![Arc(12.0, 12.0, 8.0, 270.0, 540.0)];
                strokes.extend(path(&[(15.0, 1.5), (11.5, 4.0), (15.0, 6.5)]));
                strokes
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_stays_inside_its_grid() {
        // Geometry outside the 24-unit box is drawn outside the rectangle the icon was given, over
        // whatever sits beside it — a tab's close button over its neighbour's name.
        let inside = |x: f32, y: f32| (0.0..=24.0).contains(&x) && (0.0..=24.0).contains(&y);
        for icon in Icon::ALL {
            let strokes = icon.strokes();
            assert!(!strokes.is_empty(), "{icon:?} draws nothing");
            for stroke in strokes {
                let ok = match stroke {
                    Line(x1, y1, x2, y2) => inside(x1, y1) && inside(x2, y2),
                    Ring(x, y, r) | Disc(x, y, r) => inside(x - r, y - r) && inside(x + r, y + r),
                    // Only the part that is drawn: half a circle may hang off the grid by design.
                    Arc(x, y, r, start, end) => (0..=16).all(|step| {
                        let angle = (start + (end - start) * step as f32 / 16.0).to_radians();
                        inside(x + r * angle.cos(), y + r * angle.sin())
                    }),
                    Triangle(x1, y1, x2, y2, x3, y3) => {
                        inside(x1, y1) && inside(x2, y2) && inside(x3, y3)
                    }
                };
                assert!(ok, "{icon:?} has {stroke:?} outside the grid");
            }
        }
    }
}

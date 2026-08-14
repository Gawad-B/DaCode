//! Where the chrome puts things, as data.
//!
//! Hit testing needs the same rectangles drawing uses. Two copies of that arithmetic would drift the
//! first time a panel changed size, and the symptom — clicks landing a line or two off — is the kind
//! of thing that gets blamed on the display map rather than on the duplication that caused it.
//!
//! So the geometry lives here, `draw` reads it, and the mouse reads it.

use vex_render::Rect;

/// The regions of the window, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub title: Rect,
    pub tabs: Rect,
    pub status: Rect,
    /// The editor area, before it is divided into panes.
    pub editor: Rect,
    pub sidebar: Rect,
    /// The search-results panel, when it is open.
    pub results: Option<Rect>,
}

/// A clickable button in the toolbar.
///
/// The toolbar exists because a keyboard-first editor that is *only* reachable by keyboard is an
/// editor most people close. Someone who chose the conventional editor should be able to open a
/// file, show the tree, search, save and reach the settings without having been told a single
/// shortcut — and then learn the shortcut from the button's own tooltip when they are ready.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ToolButton {
    /// The command it runs, by the name in `vex_input::commands`.
    pub command: &'static str,
    /// The short word drawn on it.
    ///
    /// A word rather than an icon glyph: the bundled font has no icon set, and an icon that renders
    /// as a tofu box teaches nothing. Words also say what they do in the user's language, which a
    /// hieroglyph of a floppy disk has not done for twenty years.
    pub label: &'static str,
}

/// The toolbar, left to right.
///
/// Ordered by how often someone reaches for it, not by category: open, then the tree, then find,
/// then save, then the two ways of finding out what else there is.
pub const TOOLBAR: [ToolButton; 7] = [
    ToolButton {
        command: "open_folder",
        label: "open",
    },
    ToolButton {
        command: "toggle_tree",
        label: "files",
    },
    ToolButton {
        command: "search",
        label: "find",
    },
    ToolButton {
        command: "save",
        label: "save",
    },
    ToolButton {
        command: "settings",
        label: "settings",
    },
    ToolButton {
        command: "command_palette",
        label: "commands",
    },
    ToolButton {
        command: "help",
        label: "keys",
    },
];

/// Where each toolbar button is, given the tab strip it sits in.
///
/// Right-aligned, because the left of that row is the file's tabs and they are the thing you are
/// looking at. One definition, read by the drawing code and by the mouse — the rule this module
/// exists for.
///
/// A narrow window cannot hold all seven, and the first version simply subtracted the full width
/// from the right edge — so in a window around 400px the leftmost buttons were laid out at negative
/// x and drawn off the side of the window, which is what a real Windows run showed. Buttons that do
/// not fit are dropped from the end instead: [`TOOLBAR`] is ordered by how often each is reached
/// for, so the tail is the part that can be spared, and a shorter toolbar is honest where half a
/// button hanging past the edge is not.
pub fn toolbar_buttons(tabs: Rect, scale: f32) -> Vec<(ToolButton, Rect)> {
    let height = 22.0 * scale;
    let gap = 6.0 * scale;
    let margin = 16.0 * scale;
    let y = tabs.y + (tabs.height - height) / 2.0;

    // Width follows the label, so nothing is clipped and the row stays tight.
    let widths: Vec<f32> = TOOLBAR
        .iter()
        .map(|button| (button.label.chars().count() as f32 + 2.0) * 7.5 * scale)
        .collect();

    let span = |count: usize| -> f32 {
        widths[..count].iter().sum::<f32>() + gap * (count.saturating_sub(1) as f32)
    };

    let available = (tabs.width - margin * 2.0).max(0.0);
    let mut count = TOOLBAR.len();
    while count > 0 && span(count) > available {
        count -= 1;
    }

    let mut x = tabs.right() - margin - span(count);
    let mut placed = Vec::with_capacity(count);
    for (button, width) in TOOLBAR.iter().take(count).zip(&widths) {
        placed.push((*button, Rect::new(x, y, *width, height)));
        x += width + gap;
    }
    placed
}

impl Layout {
    /// Computes the layout for a window of this size. The single definition of the chrome's stack.
    pub fn of(bounds: Rect, scale: f32, results_rows: Option<usize>) -> Self {
        let px = |v: f32| v * scale;

        let (title, rest) = bounds.split_top(px(72.0));
        let (tabs, rest) = rest.split_top(px(34.0));
        let (status, body) = rest.split_bottom(px(34.0));

        let (body, results) = match results_rows {
            Some(rows) => {
                let height = (rows.max(1) as f32 + 2.0) * 22.0 * scale;
                let (panel, rest) = body.split_bottom(height.min(body.height * 0.5));
                (rest, Some(panel))
            }
            None => (body, None),
        };

        let sidebar_width = px(292.0).min(body.width * 0.32);
        let (sidebar, editor) = body.split_right(sidebar_width);

        Self {
            title,
            tabs,
            status,
            editor,
            sidebar,
            results,
        }
    }

    /// The slot a pane occupies, and its gutter and code areas.
    ///
    /// The same division `draw_panes` makes, so a click in the third pane of a split lands in the
    /// third pane's text.
    pub fn pane(&self, index: usize, count: usize, vertical: bool, scale: f32) -> PaneRects {
        let count = count.max(1);
        let editor = self.editor;

        let slot = if vertical {
            let width = (editor.width / count as f32).floor();
            let x = editor.x + width * index as f32;
            Rect::new(
                x,
                editor.y,
                if index + 1 == count {
                    editor.right() - x
                } else {
                    width
                },
                editor.height,
            )
        } else {
            let height = (editor.height / count as f32).floor();
            let y = editor.y + height * index as f32;
            Rect::new(
                editor.x,
                y,
                editor.width,
                if index + 1 == count {
                    editor.bottom() - y
                } else {
                    height
                },
            )
        };

        // The hairline between panes, on the leading edge of every pane after the first.
        let slot = if index == 0 {
            slot
        } else if vertical {
            Rect::new(slot.x + scale, slot.y, slot.width - scale, slot.height)
        } else {
            Rect::new(slot.x, slot.y + scale, slot.width, slot.height - scale)
        };

        // The footer naming the file, present only when there is more than one pane.
        let (footer, slot) = if count > 1 {
            let (footer, rest) = slot.split_bottom(22.0 * scale);
            (Some(footer), rest)
        } else {
            (None, slot)
        };

        let gutter_width = (78.0 * scale).min(slot.width * 0.22);
        let (gutter, code) = slot.split_left(gutter_width);

        PaneRects {
            slot,
            gutter,
            code,
            footer,
        }
    }

    /// Which pane a point is in, if any.
    pub fn pane_at(
        &self,
        x: f32,
        y: f32,
        count: usize,
        vertical: bool,
        scale: f32,
    ) -> Option<usize> {
        (0..count.max(1)).find(|index| {
            let rects = self.pane(*index, count, vertical, scale);
            let slot = rects.slot;
            x >= slot.x && x < slot.right() && y >= slot.y && y < slot.bottom()
        })
    }
}

/// One pane's rectangles.
#[derive(Debug, Clone, Copy)]
pub struct PaneRects {
    /// Everything but the footer.
    pub slot: Rect,
    pub gutter: Rect,
    pub code: Rect,
    pub footer: Option<Rect>,
}

/// Whether a point is inside a rectangle.
pub fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::new(0.0, 0.0, 1600.0, 1000.0)
    }

    #[test]
    fn the_stack_partitions_the_window_exactly() {
        let layout = Layout::of(bounds(), 1.0, None);

        assert_eq!(layout.title.y, 0.0);
        assert_eq!(layout.tabs.y, layout.title.bottom());
        assert_eq!(layout.editor.y, layout.tabs.bottom());
        assert_eq!(layout.status.bottom(), 1000.0);
        assert_eq!(
            layout.editor.bottom(),
            layout.status.y,
            "no gap between the editor area and the statusline"
        );
        assert_eq!(
            layout.editor.right(),
            layout.sidebar.x,
            "and none between the code and the sidebar"
        );
    }

    #[test]
    fn a_narrow_window_drops_toolbar_buttons_rather_than_drawing_them_off_the_edge() {
        // A real Windows run in a narrow window laid the first buttons out at a negative x, so
        // `open` and `files` were painted past the left edge — and since the mouse reads the same
        // rectangles, they could not be clicked either. Every button that is drawn must be inside
        // the strip it belongs to, at every width.
        for width in [120.0, 200.0, 320.0, 434.0, 700.0, 1600.0] {
            let tabs = Rect::new(0.0, 72.0, width, 34.0);
            for (button, box_) in toolbar_buttons(tabs, 1.0) {
                assert!(
                    box_.x >= tabs.x,
                    "`{}` starts left of the strip in a {width}px window",
                    button.label
                );
                assert!(
                    box_.right() <= tabs.right(),
                    "`{}` runs past the right edge in a {width}px window",
                    button.label
                );
            }
        }

        let wide = toolbar_buttons(Rect::new(0.0, 72.0, 1600.0, 34.0), 1.0);
        assert_eq!(wide.len(), TOOLBAR.len(), "a wide window shows all of it");
        assert_eq!(
            wide[0].0.command,
            TOOLBAR[0].command,
            "and shows it in order"
        );

        let narrow = toolbar_buttons(Rect::new(0.0, 72.0, 200.0, 34.0), 1.0);
        assert!(narrow.len() < TOOLBAR.len(), "a narrow one cannot");
        assert_eq!(
            narrow.first().map(|(button, _)| button.command),
            Some(TOOLBAR[0].command),
            "what survives is the front of the row — the buttons reached for most"
        );
    }

    #[test]
    fn panes_tile_the_editor_area_without_gaps_or_overlap() {
        let layout = Layout::of(bounds(), 1.0, None);

        for count in 1..=4 {
            let mut covered = 0.0;
            let mut previous_right = layout.editor.x;
            for index in 0..count {
                let rects = layout.pane(index, count, true, 1.0);
                assert!(
                    rects.slot.x >= previous_right - 1.0,
                    "pane {index} of {count} overlaps the one before it"
                );
                previous_right = rects.slot.right();
                covered += rects.slot.width;
            }
            assert!(
                (covered - layout.editor.width).abs() <= count as f32,
                "{count} panes cover {covered} of {}",
                layout.editor.width
            );
        }
    }

    #[test]
    fn a_point_maps_back_to_the_pane_it_is_in() {
        let layout = Layout::of(bounds(), 1.0, None);

        for count in 1..=3 {
            for index in 0..count {
                let rects = layout.pane(index, count, true, 1.0);
                let middle_x = rects.slot.x + rects.slot.width / 2.0;
                let middle_y = rects.slot.y + rects.slot.height / 2.0;
                assert_eq!(
                    layout.pane_at(middle_x, middle_y, count, true, 1.0),
                    Some(index),
                    "the middle of pane {index} of {count} is in pane {index}"
                );
            }
        }
    }

    #[test]
    fn the_results_panel_takes_space_from_the_editor_not_the_statusline() {
        let plain = Layout::of(bounds(), 1.0, None);
        let with_panel = Layout::of(bounds(), 1.0, Some(6));

        assert!(with_panel.results.is_some());
        assert!(with_panel.editor.height < plain.editor.height);
        assert_eq!(
            with_panel.status, plain.status,
            "the statusline does not move"
        );
    }

    #[test]
    fn only_a_split_reserves_a_footer() {
        let layout = Layout::of(bounds(), 1.0, None);
        assert!(layout.pane(0, 1, true, 1.0).footer.is_none());
        assert!(layout.pane(0, 2, true, 1.0).footer.is_some());
    }
}

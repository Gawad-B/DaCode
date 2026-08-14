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

/// The mapping between a pane's code area and the display grid inside it.
///
/// **The single definition of where a column is in pixels and which column a pixel is in.**
/// `draw_code` reads it to place every glyph, underline and highlight; `App::offset_at` reads it to
/// turn a click into an offset. There were two copies of that mapping before this existed, and they
/// already disagreed — drawing padded by `space::SM` (12) and the mouse subtracted 8, so a click
/// landed most of a character out. Adding a horizontal scroll offset to two copies would have made
/// a click land in a different column from the one under the pointer the moment the view moved.
#[derive(Debug, Clone, Copy)]
pub struct CodeGrid {
    /// The code area itself, after sticky scroll has taken its share.
    pub code: Rect,
    pad: f32,
    advance: f32,
    row_height: f32,
    /// Leftmost visible column. Always 0 while soft wrap is on.
    offset: usize,
}

impl CodeGrid {
    pub fn new(code: Rect, advance: f32, row_height: f32, offset: usize, scale: f32) -> Self {
        Self {
            code,
            pad: vex_theme::space::SM * scale,
            advance,
            row_height,
            offset,
        }
    }

    pub fn advance(&self) -> f32 {
        self.advance
    }

    pub fn row_height(&self) -> f32 {
        self.row_height
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The left edge of a display column, in pixels.
    pub fn x_of(&self, column: usize) -> f32 {
        self.code.x + self.pad + (column as f32 - self.offset as f32) * self.advance
    }

    /// Which display column a pixel is in. The exact inverse of [`CodeGrid::x_of`].
    pub fn column_at(&self, x: f32) -> usize {
        let columns = ((x - self.code.x - self.pad) / self.advance).round();
        (columns.max(0.0) as usize) + self.offset
    }

    /// Which visible row a pixel is in, counted from the top of the code area.
    pub fn row_at(&self, y: f32) -> usize {
        ((y - self.code.y) / self.row_height).floor().max(0.0) as usize
    }

    /// How many whole columns fit across the code area.
    pub fn columns(&self) -> usize {
        (((self.code.width - self.pad * 2.0) / self.advance).floor()).max(1.0) as usize
    }
}

/// One open file in the tab strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tab {
    /// Index into the buffer list, which is what `switch_to` and `close_buffer` count in.
    pub index: usize,
    /// The whole tab: clicking it switches to that file.
    pub rect: Rect,
    /// The close button at its right end. Inside `rect`, so it must be tested *first*.
    pub close: Rect,
}

/// Where each open file's tab is.
///
/// The strip drew exactly one tab — the active file's name — which meant the other open buffers
/// were invisible and the only way between them was `ctrl-b` or the palette. One definition, read
/// by the drawing code and by the mouse, for the reason this module exists.
///
/// Tabs share whatever the toolbar leaves. When they no longer fit they shrink to a floor and then
/// stop being produced, rather than sliding under the buttons: a tab you cannot read is still a tab
/// you can click, but one drawn beneath something else is a click that lands on the wrong thing.
pub fn tabs(strip: Rect, count: usize, active: usize, scale: f32) -> Vec<Tab> {
    const IDEAL: f32 = 150.0;
    const FLOOR: f32 = 64.0;

    if count == 0 {
        return Vec::new();
    }

    let pad = 16.0 * scale;
    let height = 22.0 * scale;
    let y = strip.y + (strip.height - height) / 2.0;

    // The toolbar owns the right end of the same row.
    let toolbar_left = toolbar_buttons(strip, scale)
        .first()
        .map(|(_, box_)| box_.x)
        .unwrap_or(strip.right());
    let available = (toolbar_left - strip.x - pad * 2.0).max(0.0);
    if available < FLOOR * scale {
        return Vec::new();
    }

    let width = (available / count as f32).clamp(FLOOR * scale, IDEAL * scale);
    let fits = ((available / width).floor() as usize).max(1).min(count);

    // When they do not all fit, keep the window that contains the active one — the file you are
    // looking at must always have a tab.
    let first = if active >= fits { active + 1 - fits } else { 0 };

    (first..(first + fits).min(count))
        .enumerate()
        .map(|(slot, index)| {
            let rect = Rect::new(strip.x + pad + slot as f32 * width, y, width, height);
            let button = 18.0 * scale;
            Tab {
                index,
                rect,
                close: Rect::new(rect.right() - button, y, button, height),
            }
        })
        .collect()
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
            wide[0].0.command, TOOLBAR[0].command,
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
    fn a_column_and_the_pixel_it_sits_at_are_exact_inverses() {
        // The defect this exists for: drawing and hit-testing each had their own copy of this
        // mapping, and they already disagreed about the left pad — drawing used 12, the mouse used
        // 8 — so a click landed most of a character out before any horizontal scroll existed.
        // Adding an offset to two copies would have made it a whole column, and growing.
        let code = Rect::new(300.0, 120.0, 900.0, 600.0);

        for offset in [0usize, 1, 17, 240] {
            let grid = CodeGrid::new(code, 8.1, 26.0, offset, 1.0);
            for column in offset..offset + 40 {
                assert_eq!(
                    grid.column_at(grid.x_of(column)),
                    column,
                    "column {column} at offset {offset} did not survive the round trip"
                );
            }
            assert!(
                grid.x_of(offset) >= code.x,
                "the leftmost visible column must not be drawn left of the code area"
            );
        }
    }

    #[test]
    fn scrolling_sideways_moves_the_pixels_and_nothing_else() {
        let code = Rect::new(0.0, 0.0, 800.0, 400.0);
        let still = CodeGrid::new(code, 8.0, 26.0, 0, 1.0);
        let moved = CodeGrid::new(code, 8.0, 26.0, 10, 1.0);

        assert_eq!(
            moved.x_of(10),
            still.x_of(0),
            "column 10 at an offset of 10 sits where column 0 sat"
        );
        assert_eq!(
            still.row_at(30.0),
            moved.row_at(30.0),
            "the vertical axis is untouched"
        );
        assert_eq!(still.columns(), moved.columns());
    }

    #[test]
    fn every_tab_is_clickable_and_none_reaches_the_toolbar() {
        // The strip is shared with the toolbar, and both are read by the mouse. A tab drawn under a
        // button is a click that runs the wrong thing — the failure this module exists to prevent,
        // and the one that gets blamed on everything else.
        let strip = Rect::new(0.0, 72.0, 1600.0, 34.0);
        let toolbar_left = toolbar_buttons(strip, 1.0)
            .first()
            .map(|(_, box_)| box_.x)
            .expect("a wide strip has a toolbar");

        for count in 1..=12 {
            for active in 0..count {
                let tabs = tabs(strip, count, active, 1.0);
                assert!(
                    tabs.iter().any(|tab| tab.index == active),
                    "the file being edited must always have a tab: {active} of {count}"
                );
                for tab in &tabs {
                    assert!(tab.rect.x >= strip.x, "a tab starts left of the strip");
                    assert!(
                        tab.rect.right() <= toolbar_left,
                        "a tab runs under the toolbar with {count} open"
                    );
                    assert!(
                        contains(tab.rect, tab.close.x + 1.0, tab.close.y + 1.0),
                        "the close button must sit inside its own tab"
                    );
                }
                for pair in tabs.windows(2) {
                    assert!(
                        pair[0].rect.right() <= pair[1].rect.x + 0.01,
                        "two tabs overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn a_narrow_strip_gives_up_on_tabs_rather_than_drawing_unreadable_ones() {
        // At some width there is no honest tab to draw. Returning none is better than a sliver that
        // cannot show a name but can still be clicked.
        let tabs = tabs(Rect::new(0.0, 72.0, 120.0, 34.0), 5, 0, 1.0);
        assert!(tabs.is_empty());
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

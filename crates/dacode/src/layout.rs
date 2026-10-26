//! Where the chrome puts things, as data.
//!
//! Hit testing needs the same rectangles drawing uses. Two copies of that arithmetic would drift the
//! first time a panel changed size, and the symptom — clicks landing a line or two off — is the kind
//! of thing that gets blamed on the display map rather than on the duplication that caused it.
//!
//! So the geometry lives here, `draw` reads it, and the mouse reads it.

use dacode_render::Rect;

/// The regions of the window, in physical pixels.
///
/// The shape VSCode and Zed share, because it is the shape people arrive knowing: a title bar across
/// the top, an icon column down the left that chooses what the sidebar shows, the sidebar, and the
/// editor column — tabs, panes, and a status bar under them.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub title: Rect,
    /// The column of activity icons.
    pub activity: Rect,
    /// The sidebar, when one is open. Closed, the editor takes its width — as it does in both
    /// editors this one is shaped after.
    pub sidebar: Option<Rect>,
    /// How wide the sidebar is when it is fully open, whether or not it is now. A sidebar sliding
    /// in lays its contents out at this width and shows the part that has arrived, so the rows do
    /// not reflow on every frame of the slide.
    pub sidebar_width: f32,
    pub tabs: Rect,
    pub status: Rect,
    /// The editor area, before it is divided into panes.
    pub editor: Rect,
    /// The search-results panel, when it is open.
    pub results: Option<Rect>,
}

/// Sizes of the chrome, in logical pixels.
pub mod size {
    pub const TITLE: f32 = 48.0;
    pub const ACTIVITY: f32 = 52.0;
    pub const SIDEBAR: f32 = 252.0;
    pub const TABS: f32 = 40.0;
    pub const STATUS: f32 = 30.0;
    /// An icon button's square.
    pub const BUTTON: f32 = 30.0;
    /// The icon inside it.
    pub const ICON: f32 = 18.0;
}

/// One of the views the activity bar chooses between.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Activity {
    Explorer,
    Search,
    SourceControl,
    Debug,
    Problems,
    Settings,
}

/// An icon in the activity bar: what it shows, and the command clicking it runs.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub activity: Activity,
    pub icon: dacode_render::Icon,
    /// By the name in `dacode_input::commands`, so the icon runs exactly what the key does and its
    /// tooltip can name that key.
    pub command: &'static str,
    pub label: &'static str,
}

/// The activity bar, top to bottom. The last is pinned to the bottom of the column, apart from the
/// others, as both editors place their settings.
pub const ACTIVITIES: [ActivityItem; 6] = [
    ActivityItem {
        activity: Activity::Explorer,
        icon: dacode_render::Icon::Explorer,
        command: "toggle_tree",
        label: "Explorer",
    },
    ActivityItem {
        activity: Activity::Search,
        icon: dacode_render::Icon::Search,
        command: "search_project",
        label: "Search",
    },
    ActivityItem {
        activity: Activity::SourceControl,
        icon: dacode_render::Icon::Branch,
        command: "source_control",
        label: "Source control",
    },
    ActivityItem {
        activity: Activity::Debug,
        icon: dacode_render::Icon::Bug,
        command: "run_and_debug",
        label: "Run and debug",
    },
    ActivityItem {
        activity: Activity::Problems,
        icon: dacode_render::Icon::Bulb,
        command: "toggle_diagnostics",
        label: "Problems",
    },
    ActivityItem {
        activity: Activity::Settings,
        icon: dacode_render::Icon::Gear,
        command: "settings",
        label: "Settings",
    },
];

/// Where each activity icon is. Squares down the column, the last one at the bottom.
///
/// A column too short for all of them keeps the bottom one and drops from the end of the rest,
/// rather than letting the two groups overlap — an icon under another is a click on the wrong view.
pub fn activity_items(bar: Rect, scale: f32) -> Vec<(ActivityItem, Rect)> {
    let cell = bar.width.min(48.0 * scale);
    let x = bar.x + (bar.width - cell) / 2.0;
    let top = bar.y + 6.0 * scale;
    let (last, rest) = ACTIVITIES.split_last().expect("the table is not empty");
    let bottom = Rect::new(x, bar.bottom() - cell - 8.0 * scale, cell, cell);

    let mut placed: Vec<(ActivityItem, Rect)> = rest
        .iter()
        .enumerate()
        .map(|(index, item)| (*item, Rect::new(x, top + index as f32 * cell, cell, cell)))
        .take_while(|(_, rect)| rect.bottom() <= bottom.y)
        .collect();
    if bottom.y >= bar.y {
        placed.push((*last, bottom));
    }
    placed
}

/// What the title bar holds, left to right.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleBar {
    pub logo: Rect,
    /// The wordmark. Zero wide on a bar too narrow to spare it, where the menus matter more.
    pub name: Rect,
    /// One rectangle per menu title that fits, in the order of [`crate::menu::TITLES`]: always a
    /// prefix of it, because the titles that do not fit drop from the end.
    pub menus: Vec<Rect>,
    /// "…", standing for the titles that did not fit. Absent when they all did.
    pub overflow: Option<Rect>,
    /// The command centre: a search box that opens the palette. Absent when the bar is too narrow
    /// to hold it beside the first menu.
    pub search: Option<Rect>,
    pub bell: Rect,
}

/// The overflow title's text.
pub const OVERFLOW: &str = "…";

/// The strip across the top of the window that the title bar fills.
///
/// One definition, read by [`Layout::of`] and by `menu::geometry`: the titles are centred on this
/// height, and a menu that measured its own strip would hang its card under a title that is not
/// where the bar drew it.
pub fn title_strip(bounds: Rect, scale: f32) -> Rect {
    bounds.split_top((size::TITLE * scale).min(bounds.height)).0
}

/// What the title bar may hold besides the mark and the bell, in the order room is handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// A menu title, by its index in [`crate::menu::TITLES`].
    Menu(usize),
    Search,
    Wordmark,
}

/// The fitting rule, as one list: an element is on the bar only if everything before it is and it
/// fits too. File, Edit and Selection come first because a bar without them is not a menu bar;
/// the search box next, because the palette is what the keyboard-minded reach for; then the other
/// menus; the wordmark last, because it is the one thing on the bar that does nothing.
///
/// A rule shaped this way cannot drop a control as the window widens: room only grows with width,
/// and what is shown is always a longer prefix of the same list. The rule before it gave the
/// search box and the wordmark priority by threshold, and both thresholds took titles away from a
/// window that had just been made wider.
const PRIORITY: [Part; 10] = [
    Part::Menu(0),
    Part::Menu(1),
    Part::Menu(2),
    Part::Search,
    Part::Menu(3),
    Part::Menu(4),
    Part::Menu(5),
    Part::Menu(6),
    Part::Menu(7),
    Part::Wordmark,
];

/// The title bar's controls, for drawing and for the mouse.
///
/// Room between the mark and the bell is handed out along [`PRIORITY`]. While any title is left
/// out the overflow's width is held back for "…", which opens the rest. The search box, once on
/// the bar, grows into whatever is left — up to 440 pixels, centred on the window when it can be.
///
/// Titles are sized from the `Ui` role's advance, with slack, because that is an estimate of the
/// shaper rather than the shaper — a title a character too narrow would be cut mid-word.
pub fn title_bar(title: Rect, scale: f32, typography: &dacode_theme::Typography) -> TitleBar {
    let px = |v: f32| v * scale;
    let centre_y = |height: f32| title.y + (title.height - height) / 2.0;

    let logo = Rect::new(title.x + px(16.0), centre_y(px(28.0)), px(28.0), px(28.0));
    let button = px(size::BUTTON);
    let bell = Rect::new(
        title.right() - px(16.0) - button,
        centre_y(button),
        button,
        button,
    );

    let advance = typography.advance(dacode_theme::Role::Ui) * scale * 1.1;
    let measure = |text: &str| (text.chars().count() as f32 * advance + px(16.0)).ceil();
    let widths: Vec<f32> = crate::menu::TITLES.iter().map(|t| measure(t)).collect();
    let ellipsis = measure(OVERFLOW);
    let gap = px(16.0);
    let narrowest_search = px(180.0);
    let wordmark = px(10.0 + 96.0);
    let cost = |part: Part| match part {
        Part::Menu(index) => widths[index],
        Part::Search => narrowest_search + gap,
        Part::Wordmark => wordmark,
    };

    let start = logo.right() + px(8.0);
    let end = bell.x - px(16.0);
    let room = (end - start).max(0.0);

    // The longest prefix of the list that fits, holding the overflow's room while it is short of
    // a title. Adding the last title releases that room, but the title is wider than "…", so the
    // cost still only grows along the list and the prefix only lengthens with the window.
    let mut taken = 0;
    let mut used = 0.0;
    for (index, part) in PRIORITY.iter().enumerate() {
        let after = used + cost(*part);
        let missing_a_title = PRIORITY[index + 1..]
            .iter()
            .any(|later| matches!(later, Part::Menu(_)));
        let needed = if missing_a_title {
            after + ellipsis
        } else {
            after
        };
        if needed > room {
            break;
        }
        used = after;
        taken = index + 1;
    }
    let chosen = &PRIORITY[..taken];
    let shown = chosen
        .iter()
        .filter(|part| matches!(part, Part::Menu(_)))
        .count();

    let name = if chosen.contains(&Part::Wordmark) {
        Rect::new(logo.right() + px(10.0), title.y, px(96.0), title.height)
    } else {
        Rect::new(logo.right(), title.y, 0.0, title.height)
    };

    let height = px(28.0);
    let mut x = name.right() + px(8.0);
    let menus: Vec<Rect> = widths[..shown]
        .iter()
        .map(|width| {
            let rect = Rect::new(x, centre_y(height), *width, height);
            x += width;
            rect
        })
        .collect();
    let overflow = (shown < widths.len() && x + ellipsis <= end + 0.5).then(|| {
        let rect = Rect::new(x, centre_y(height), ellipsis, height);
        x += ellipsis;
        rect
    });

    let search = chosen.contains(&Part::Search).then(|| {
        let left = x + gap;
        let width = (end - left).min(px(440.0)).max(0.0);
        let middle = title.x + title.width / 2.0;
        let x = (middle - width / 2.0).max(left).min(end - width);
        Rect::new(x, centre_y(px(28.0)), width, px(28.0))
    });

    TitleBar {
        logo,
        name,
        menus,
        overflow,
        search,
        bell,
    }
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
    /// How far the top row has scrolled up out of view, in pixels: the part of a touchpad's motion
    /// that has not yet made a whole row. The vertical counterpart of `offset`, and like it, it
    /// enters pixel arithmetic here and nowhere else.
    sub_row: f32,
}

impl CodeGrid {
    pub fn new(code: Rect, advance: f32, row_height: f32, offset: usize, scale: f32) -> Self {
        Self {
            code,
            pad: dacode_theme::space::SM * scale,
            advance,
            row_height,
            offset,
            sub_row: 0.0,
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

    /// Which visible row a pixel is in, counted from the top of the code area. A part-scrolled top
    /// row is still row zero, so a click lands on the row under the pointer.
    pub fn row_at(&self, y: f32) -> usize {
        ((y - self.code.y + self.sub_row) / self.row_height)
            .floor()
            .max(0.0) as usize
    }

    /// The same grid with its rows moved up by `px`: how far the top row has scrolled out of view.
    ///
    /// Clamped to `[0, row_height]`. A negative offset would pull the top row down and leave a gap
    /// above it; [`crate::scroll::by_pixels`] never produces one, and this refuses one anyway.
    pub fn with_sub_row(self, px: f32) -> Self {
        let sub_row = if px.is_finite() {
            px.clamp(0.0, self.row_height)
        } else {
            0.0
        };
        Self { sub_row, ..self }
    }

    /// How far the top row has scrolled up out of view, in pixels.
    pub fn sub_row(&self) -> f32 {
        self.sub_row
    }

    /// The top edge of a visible row, in pixels. The exact inverse of [`CodeGrid::row_at`].
    ///
    /// Row zero is above `code.y` when part-scrolled, so anything drawn from this must be clipped
    /// to the code area — `draw_code` sets the frame's clip for exactly that.
    pub fn y_of(&self, row: usize) -> f32 {
        self.code.y + row as f32 * self.row_height - self.sub_row
    }

    /// How many rows to draw: the ones that fit whole, and one more while part-scrolled, which is the
    /// row sliding in at the bottom as the top one slides out.
    pub fn rows_drawn(&self) -> usize {
        let whole = ((self.code.height / self.row_height).floor() as usize).max(1);
        whole + usize::from(self.sub_row > 0.0)
    }

    /// How many whole columns fit across the code area.
    pub fn columns(&self) -> usize {
        (((self.code.width - self.pad * 2.0) / self.advance).floor()).max(1.0) as usize
    }
}

/// A pane's code area once sticky scroll has taken its strip off the top.
///
/// `draw_code` used to make this split itself, shadowing its own `code` parameter — which left
/// every other reader of that rectangle measuring against one that was a row taller per pinned
/// head. The mouse was one of them, so with three heads pinned a click landed three lines below the
/// line under the pointer, and the hover box and completion popup were drawn three rows above the
/// cursor they belonged to. The split happens once now, here, and everything reads the result.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeArea {
    /// The rows that scroll. This is the rectangle to measure anything against.
    pub code: Rect,
    /// The strip holding the pinned heads, when there are any.
    pub pinned: Option<Rect>,
    /// The block heads pinned above the viewport, outermost first.
    pub heads: Vec<usize>,
}

impl CodeArea {
    pub fn new(code: Rect, heads: Vec<usize>, row_height: f32) -> Self {
        if heads.is_empty() {
            return Self {
                code,
                pinned: None,
                heads,
            };
        }
        let (pinned, rest) = code.split_top(row_height * heads.len() as f32);
        Self {
            code: rest,
            pinned: Some(pinned),
            heads,
        }
    }
}

/// One open file in the tab strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tab {
    /// Index into the buffer list, which is what `switch_to` and `close_buffer` count in.
    pub index: usize,
    /// The whole tab: clicking it switches to that file.
    pub rect: Rect,
    /// The file's icon at its left.
    pub icon: Rect,
    /// Where the name is drawn, between the icon and the close button.
    pub label: Rect,
    /// The close button at its right end. Inside `rect`, so it must be tested *first*.
    pub close: Rect,
}

/// The row above the code: navigation on the left, the open files, and the pane controls on the
/// right.
#[derive(Debug, Clone, PartialEq)]
pub struct TabBar {
    pub home: Rect,
    pub back: Rect,
    pub forward: Rect,
    pub tabs: Vec<Tab>,
    pub split: Rect,
    pub run: Rect,
}

/// The buttons the tab bar carries besides the tabs, and the commands they run.
pub const TAB_BAR_COMMANDS: [(&str, dacode_render::Icon, &str); 5] = [
    ("open_folder", dacode_render::Icon::Home, "Open a folder"),
    ("jump_back", dacode_render::Icon::ChevronLeft, "Back"),
    ("jump_forward", dacode_render::Icon::ChevronRight, "Forward"),
    (
        "split_vertical",
        dacode_render::Icon::Split,
        "Split the editor",
    ),
    ("run", dacode_render::Icon::Play, "Run"),
];

/// Where everything in the tab bar is.
///
/// Each tab is as wide as its name needs, between a floor and a ceiling, the way both VSCode and Zed
/// size them. When they do not all fit, the run that holds the active tab is kept — the file being
/// edited must always have a tab — and the others are left out rather than squeezed below the
/// width where a name can be read, or slid under the buttons, where a click would land on the wrong
/// thing. `names` is one entry per open buffer.
pub fn tab_bar(
    strip: Rect,
    names: &[&str],
    active: usize,
    typography: &dacode_theme::Typography,
    scale: f32,
) -> TabBar {
    let px = |v: f32| v * scale;
    let button = px(size::BUTTON);
    let y = strip.y + (strip.height - button) / 2.0;

    let home = Rect::new(strip.x + px(10.0), y, button, button);
    let back = Rect::new(home.right() + px(8.0), y, button, button);
    let forward = Rect::new(back.right() + px(2.0), y, button, button);
    let run = Rect::new(strip.right() - px(10.0) - button, y, button, button);
    let split = Rect::new(run.x - px(6.0) - button, y, button, button);

    let left = forward.right() + px(12.0);
    let right = split.x - px(12.0);
    let height = px(30.0);
    let tab_y = strip.y + (strip.height - height) / 2.0;

    // With slack: `advance` is a type-scale estimate, not the shaper, and a name a character too
    // wide for its box is cut to an ellipsis it did not need.
    let advance = typography.advance(dacode_theme::Role::Ui) * scale * 1.15;
    // Padding, icon, gap — then gap, close button, padding. The same terms the rectangles below
    // are built from, so the name's box is exactly what is left.
    let chrome = px(12.0 + 16.0 + 8.0 + 8.0 + 18.0 + 10.0);
    let widths: Vec<f32> = names
        .iter()
        .map(|name| {
            (name.chars().count() as f32 * advance + chrome)
                .clamp(px(96.0), px(220.0))
                .ceil()
        })
        .collect();
    let gap = px(4.0);

    // Grow a window of tabs outwards from the active one until the next would not fit.
    let active = active.min(names.len().saturating_sub(1));
    let available = (right - left).max(0.0);
    let (mut first, mut last) = (active, active);
    let mut used = widths.get(active).copied().unwrap_or(0.0);
    if names.is_empty() || used > available {
        return TabBar {
            home,
            back,
            forward,
            tabs: Vec::new(),
            split,
            run,
        };
    }
    loop {
        let after = (last + 1 < names.len()).then(|| widths[last + 1] + gap);
        let before = (first > 0).then(|| widths[first - 1] + gap);
        match (after, before) {
            (Some(width), _) if used + width <= available => {
                last += 1;
                used += width;
            }
            (_, Some(width)) if used + width <= available => {
                first -= 1;
                used += width;
            }
            _ => break,
        }
    }

    let mut x = left;
    let tabs = (first..=last)
        .map(|index| {
            let rect = Rect::new(x, tab_y, widths[index], height);
            x += widths[index] + gap;
            let icon_size = px(16.0);
            let icon = Rect::new(
                rect.x + px(12.0),
                rect.y + (height - icon_size) / 2.0,
                icon_size,
                icon_size,
            );
            let close = Rect::new(
                rect.right() - px(10.0) - px(18.0),
                rect.y + (height - px(18.0)) / 2.0,
                px(18.0),
                px(18.0),
            );
            let label = Rect::new(
                icon.right() + px(8.0),
                rect.y,
                (close.x - px(8.0) - (icon.right() + px(8.0))).max(0.0),
                height,
            );
            Tab {
                index,
                rect,
                icon,
                label,
                close,
            }
        })
        .collect();

    TabBar {
        home,
        back,
        forward,
        tabs,
        split,
        run,
    }
}

impl Layout {
    /// Computes the layout for a window of this size. The single definition of the chrome's stack,
    /// read by the drawing code and by the mouse alike.
    ///
    /// The sidebar gives way first when the window is narrow: past a third of the width it would
    /// leave the code too little room to be worth editing in.
    ///
    /// `sidebar` is how far open the sidebar is, from 0 (closed) to 1 (open), and a little past 1
    /// while a spring overshoots. Drawing passes where the slide has got to and so does the mouse, so
    /// a click lands on what is on screen even mid-slide; soft wrap passes where it is going, so the
    /// text does not rewrap on every frame of it.
    pub fn of(bounds: Rect, scale: f32, results_rows: Option<usize>, sidebar: f32) -> Self {
        let px = |v: f32| v * scale;

        let title = title_strip(bounds, scale);
        let (_, rest) = bounds.split_top(title.height);
        let (activity, rest) = rest.split_left(px(size::ACTIVITY).min(rest.width));
        let sidebar_width = px(size::SIDEBAR).min(rest.width * 0.34).floor();
        let fraction = if sidebar.is_finite() {
            sidebar.max(0.0)
        } else {
            1.0
        };
        // Whole pixels, so the hairline on its edge stays one crisp column while it moves.
        let width = (sidebar_width * fraction).round();
        let (sidebar, column) = if width >= 1.0 {
            let (sidebar, column) = rest.split_left(width);
            (Some(sidebar), column)
        } else {
            (None, rest)
        };

        let (tabs, rest) = column.split_top(px(size::TABS).min(column.height));
        let (status, body) = rest.split_bottom(px(size::STATUS).min(rest.height));

        let (editor, results) = match results_rows {
            Some(rows) => {
                let height = (rows.max(1) as f32 + 2.0) * crate::results::ROW * scale;
                let (panel, rest) = body.split_bottom(height.min(body.height * 0.5));
                (rest, Some(panel))
            }
            None => (body, None),
        };

        Self {
            title,
            activity,
            sidebar,
            sidebar_width,
            tabs,
            status,
            editor,
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

/// The chips a key string is drawn as: one per *alternative*, each holding every press of its key.
///
/// Both notations separate alternatives with two spaces — Vim's table writes `:w  <c-s>`, and
/// `commands::pretty` keeps that gap — while a single space separates the presses of one key
/// (`Ctrl+K O`). So a chord is one chip with its strokes inside it, and two chips side by side
/// always mean "either of these". Splitting at every space drew `Ctrl+K O` as two equal chips,
/// identical to the two of redo's `Ctrl+Y` `Ctrl+Shift+Z`, and a reader could take the `O` for a
/// key that opens a folder by itself.
pub fn key_presses(keys: &str) -> Vec<&str> {
    keys.split("  ")
        .map(str::trim)
        .filter(|alternative| !alternative.is_empty())
        .collect()
}

/// How much wider the bundled face really sets a character than the type scale's estimate.
///
/// `Typography::advance` says 0.60 em; Fira Code's advance is 1200 units in an em of 1950, which
/// is 0.615. Two and a half percent is nothing in one word and three characters in a line of a
/// hundred and twenty — enough that text measured to fit a box exactly ran past its edge and was
/// cut mid-word by the renderer's clip, which is the one thing [`crate::chrome`]'s `elide` exists to
/// prevent. Measurements that decide whether text *fits* go through [`measured_advance`]; the code
/// grid does not, since moving it is a change to where every glyph of code is drawn.
pub const FACE_ADVANCE: f32 = FACE_EM / 0.60;

/// The bundled face's advance, in ems: 1200 units of 1950.
pub const FACE_EM: f32 = 1200.0 / 1950.0;

/// The width one character of `role` really takes, for deciding what fits.
pub fn measured_advance(
    typography: &dacode_theme::Typography,
    role: dacode_theme::Role,
    scale: f32,
) -> f32 {
    typography.advance(role) * scale * FACE_ADVANCE
}

/// [`measured_advance`] for text set at a size of its own rather than a role's — the wizard sets
/// some of its text off the type scale. Untracked, as every role but the labels is.
pub fn measured_advance_at(size_px: f32) -> f32 {
    size_px * FACE_EM
}

/// Padding either side of a key chip's text, and the gap between two chips, at scale 1.
pub const CHIP_PAD: f32 = 5.0;
pub const CHIP_GAP: f32 = 4.0;

/// How wide `keys` is when drawn as chips, measured with [`measured_advance`] like the chips
/// themselves, so a menu sized by this is the width the chips are drawn at.
pub fn chips_width(typography: &dacode_theme::Typography, keys: &str, scale: f32) -> f32 {
    let advance = measured_advance(typography, dacode_theme::Role::Value, scale);
    let presses = key_presses(keys);
    let text: f32 = presses
        .iter()
        .map(|press| press.chars().count() as f32 * advance + CHIP_PAD * 2.0 * scale)
        .sum();
    text + presses.len().saturating_sub(1) as f32 * CHIP_GAP * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_is_one_chip_and_alternatives_are_several() {
        // One chip holds every stroke of one key; chips side by side are alternatives.
        assert_eq!(key_presses("Ctrl+K O"), vec!["Ctrl+K O"]);
        assert_eq!(
            key_presses("Ctrl+Y  Ctrl+Shift+Z"),
            vec!["Ctrl+Y", "Ctrl+Shift+Z"]
        );
        assert_eq!(key_presses(":w  <c-s>"), vec![":w", "<c-s>"]);
        assert_eq!(key_presses("gD"), vec!["gD"]);
        assert!(key_presses("").is_empty());
        let typography = dacode_theme::Typography::default();
        assert_eq!(chips_width(&typography, "", 1.0), 0.0);
        assert!(
            chips_width(&typography, "Ctrl+Y  Ctrl+Z", 1.0)
                > chips_width(&typography, "Ctrl+YCtrl+Z", 1.0),
            "two chips take their padding and the gap between them"
        );
    }

    fn bounds() -> Rect {
        Rect::new(0.0, 0.0, 1600.0, 1000.0)
    }

    #[test]
    fn the_regions_partition_the_window_exactly() {
        // At every point of the sidebar sliding open, not only at its ends: a frame half-way through
        // is still a frame the mouse has to agree with.
        let full = Layout::of(bounds(), 1.0, None, 1.0)
            .sidebar
            .expect("open")
            .width;
        for sidebar in [0.0, 0.5, 1.0] {
            let layout = Layout::of(bounds(), 1.0, None, sidebar);
            if sidebar == 0.0 {
                assert!(layout.sidebar.is_none(), "closed is no sidebar at all");
            } else {
                let width = layout.sidebar.expect("open part-way").width;
                assert_eq!(width, (full * sidebar).round(), "at {sidebar}");
                assert_eq!(
                    layout.sidebar_width, full,
                    "the open width is known throughout"
                );
            }

            assert_eq!(layout.title.y, 0.0);
            assert_eq!(layout.activity.y, layout.title.bottom());
            assert_eq!(
                layout.activity.bottom(),
                1000.0,
                "the icon column runs to the bottom"
            );
            let column_left = match layout.sidebar {
                Some(panel) => {
                    assert_eq!(panel.x, layout.activity.right());
                    assert_eq!(panel.bottom(), 1000.0);
                    panel.right()
                }
                None => layout.activity.right(),
            };
            assert_eq!(
                layout.tabs.x, column_left,
                "no gap before the editor column"
            );
            assert_eq!(layout.tabs.right(), 1600.0);
            assert_eq!(layout.editor.y, layout.tabs.bottom());
            assert_eq!(
                layout.editor.bottom(),
                layout.status.y,
                "no gap between the editor area and the status bar"
            );
            assert_eq!(layout.status.bottom(), 1000.0);
        }
    }

    #[test]
    fn closing_the_sidebar_gives_its_width_to_the_editor() {
        let open = Layout::of(bounds(), 1.0, None, 1.0);
        let closed = Layout::of(bounds(), 1.0, None, 0.0);
        let sidebar = open.sidebar.expect("asked for one").width;
        assert_eq!(closed.editor.width, open.editor.width + sidebar);
    }

    #[test]
    fn the_title_bars_controls_stay_inside_it_and_apart_at_every_width() {
        // Four overflow defects shipped because the harness only ever rendered at 1600x1000. These
        // are all clickable, so one drawn past the edge or over another is a click on the wrong thing.
        let typography = dacode_theme::Typography::default();
        for width in [360.0, 480.0, 640.0, 900.0, 1280.0, 1600.0, 2560.0] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let title = Rect::new(0.0, 0.0, width * scale, size::TITLE * scale);
                let bar = title_bar(title, scale, &typography);
                let mut controls = vec![bar.logo, bar.bell];
                controls.extend(bar.menus.iter().copied());
                controls.extend(bar.overflow);
                controls.extend(bar.search);
                for control in &controls {
                    assert!(
                        control.x >= title.x - 0.01 && control.right() <= title.right() + 0.01,
                        "{control:?} leaves a {width}px bar"
                    );
                    assert!(control.y >= title.y && control.bottom() <= title.bottom());
                }
                for (index, a) in controls.iter().enumerate() {
                    for b in &controls[index + 1..] {
                        assert!(
                            a.intersect(*b).width <= 0.0 || a.intersect(*b).height <= 0.0,
                            "{a:?} and {b:?} overlap at {width}px, scale {scale}"
                        );
                    }
                }
                if width >= 900.0 {
                    let search = bar.search.expect("a wide bar has the search box");
                    assert!(search.right() <= bar.bell.x);
                    assert!(search.x >= bar.name.right());
                }
            }
        }
    }

    #[test]
    fn the_menu_titles_sit_after_the_name_and_before_the_search_box() {
        // The bar reads left to right as every desktop editor's does: the mark, the name, the menus,
        // then the search box. A title under the search box is a click that opens the palette when
        // it looked like it opened a menu.
        let typography = dacode_theme::Typography::default();
        for width in [360.0, 480.0, 640.0, 900.0, 1280.0, 1600.0, 2560.0] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let title = Rect::new(0.0, 0.0, width * scale, size::TITLE * scale);
                let bar = title_bar(title, scale, &typography);
                assert!(
                    !bar.menus.is_empty(),
                    "File at least is always on the bar, at {width}px scale {scale}"
                );
                let mut row: Vec<Rect> = bar.menus.clone();
                row.extend(bar.overflow);
                assert!(
                    row[0].x >= bar.name.right(),
                    "the first title is after the name"
                );
                for pair in row.windows(2) {
                    assert!(pair[0].right() <= pair[1].x + 0.01, "titles are in order");
                }
                if let Some(search) = bar.search {
                    let last = row.last().expect("not empty");
                    assert!(
                        last.right() <= search.x,
                        "the titles end before the search box at {width}px scale {scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn trailing_menu_titles_drop_behind_an_overflow_when_they_do_not_fit() {
        // Squeezing the titles below the width a word needs would cut them mid-word; dropping from
        // the end keeps every one that shows readable, and the overflow opens the rest.
        let typography = dacode_theme::Typography::default();
        let all = crate::menu::TITLES.len();
        for width in [360.0, 480.0, 640.0, 900.0, 1280.0, 1600.0, 2560.0] {
            let bar = title_bar(Rect::new(0.0, 0.0, width, size::TITLE), 1.0, &typography);
            assert_eq!(
                bar.overflow.is_some(),
                bar.menus.len() < all,
                "the overflow is there exactly when a title is missing, at {width}px"
            );
            if width >= 1280.0 {
                assert_eq!(bar.menus.len(), all, "a wide bar shows every menu");
            }
        }
        let narrow = title_bar(Rect::new(0.0, 0.0, 360.0, size::TITLE), 1.0, &typography);
        assert!(
            narrow.menus.len() < all,
            "a phone-width bar cannot hold all eight"
        );
    }

    #[test]
    fn widening_the_window_never_takes_anything_off_the_title_bar() {
        // Dragging a window's edge outwards must not make a title vanish: the pointer heading for
        // Edit would land on whatever moved into its place. The first fitting rule gave the search
        // box and the wordmark priority by threshold, and each threshold did exactly that at one
        // width — which a sample of seven widths could not see. Every pixel, then.
        for code in [11.0, 13.5, 18.0] {
            let typography = dacode_theme::Typography::with_code_size(code);
            for scale in [1.0, 2.0] {
                let mut before = (0, false, false);
                for width in 360..=2560 {
                    let title = Rect::new(0.0, 0.0, width as f32 * scale, size::TITLE * scale);
                    let bar = title_bar(title, scale, &typography);
                    let now = (bar.menus.len(), bar.search.is_some(), bar.name.width > 0.0);
                    assert!(
                        now.0 >= before.0,
                        "titles fell from {} to {} at {width}px, scale {scale}, code {code}",
                        before.0,
                        now.0
                    );
                    assert!(
                        now.1 || !before.1,
                        "the search box left at {width}px, scale {scale}, code {code}"
                    );
                    assert!(
                        now.2 || !before.2,
                        "the wordmark left at {width}px, scale {scale}, code {code}"
                    );
                    before = now;
                }
            }
        }
    }

    #[test]
    fn settings_stays_at_the_bottom_and_no_activity_icon_covers_another() {
        for height in [120.0, 260.0, 400.0, 800.0] {
            let bar = Rect::new(0.0, 48.0, size::ACTIVITY, height);
            let placed = activity_items(bar, 1.0);
            for pair in placed.windows(2) {
                assert!(
                    pair[0].1.bottom() <= pair[1].1.y + 0.01,
                    "{:?} runs into {:?} in a {height}px column",
                    pair[0].0.activity,
                    pair[1].0.activity
                );
            }
            for (_, rect) in &placed {
                assert!(rect.y >= bar.y && rect.bottom() <= bar.bottom());
            }
            if height >= 400.0 {
                assert_eq!(
                    placed.len(),
                    ACTIVITIES.len(),
                    "a tall column shows them all"
                );
                assert_eq!(
                    placed.last().map(|(item, _)| item.activity),
                    Some(Activity::Settings)
                );
            }
        }
    }

    #[test]
    fn every_tab_is_clickable_and_none_reaches_the_buttons() {
        // The strip is shared with the navigation and pane buttons, and both are read by the mouse.
        // A tab drawn under a button is a click that runs the wrong thing.
        let typography = dacode_theme::Typography::default();
        let names = [
            "main.rs",
            "a_much_longer_file_name_than_usual.rs",
            "x",
            "lib.rs",
            "Cargo.toml",
            "README.md",
            "layout.rs",
            "chrome.rs",
            "e",
            "settings_ui.rs",
            "f",
            "g",
        ];
        for width in [500.0, 900.0, 1600.0] {
            let strip = Rect::new(300.0, 48.0, width, 40.0);
            for count in 1..=names.len() {
                for active in 0..count {
                    let bar = tab_bar(strip, &names[..count], active, &typography, 1.0);
                    assert!(
                        bar.tabs.iter().any(|tab| tab.index == active),
                        "the file being edited must always have a tab: {active} of {count} at {width}"
                    );
                    for tab in &bar.tabs {
                        assert!(
                            tab.rect.x >= bar.forward.right(),
                            "a tab under the navigation"
                        );
                        assert!(
                            tab.rect.right() <= bar.split.x,
                            "a tab under the pane buttons"
                        );
                        assert!(
                            contains(tab.rect, tab.close.x + 1.0, tab.close.y + 1.0),
                            "the close button must sit inside its own tab"
                        );
                        assert!(
                            tab.label.right() <= tab.close.x,
                            "a name under the close button"
                        );
                    }
                    for pair in bar.tabs.windows(2) {
                        assert!(
                            pair[0].rect.right() <= pair[1].rect.x + 0.01,
                            "two tabs overlap"
                        );
                        assert_eq!(pair[0].index + 1, pair[1].index, "tabs keep their order");
                    }
                }
            }
        }
    }

    #[test]
    fn a_strip_with_no_room_gives_up_on_tabs_rather_than_drawing_unreadable_ones() {
        // At some width there is no honest tab to draw. Returning none is better than a sliver that
        // cannot show a name but can still be clicked.
        let typography = dacode_theme::Typography::default();
        let bar = tab_bar(
            Rect::new(0.0, 48.0, 200.0, 40.0),
            &["main.rs"; 5],
            0,
            &typography,
            1.0,
        );
        assert!(bar.tabs.is_empty());
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
    fn a_part_scrolled_row_is_still_the_row_under_the_pointer() {
        let code = Rect::new(0.0, 100.0, 800.0, 400.0);
        let grid = CodeGrid::new(code, 8.0, 26.0, 0, 1.0).with_sub_row(13.0);
        assert_eq!(grid.row_at(code.y), 0, "the half-hidden top row");
        assert_eq!(grid.row_at(code.y + 13.0 + 1.0), 1);
        assert_eq!(grid.row_at(code.y + 13.0 + 26.0 + 1.0), 2);
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
    fn panes_tile_the_editor_area_without_gaps_or_overlap() {
        let layout = Layout::of(bounds(), 1.0, None, 1.0);

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
        let layout = Layout::of(bounds(), 1.0, None, 1.0);

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
        let plain = Layout::of(bounds(), 1.0, None, 1.0);
        let with_panel = Layout::of(bounds(), 1.0, Some(6), 1.0);

        assert!(with_panel.results.is_some());
        assert!(with_panel.editor.height < plain.editor.height);
        assert_eq!(
            with_panel.status, plain.status,
            "the statusline does not move"
        );
    }

    #[test]
    fn only_a_split_reserves_a_footer() {
        let layout = Layout::of(bounds(), 1.0, None, 1.0);
        assert!(layout.pane(0, 1, true, 1.0).footer.is_none());
        assert!(layout.pane(0, 2, true, 1.0).footer.is_some());
    }

    #[test]
    fn a_pinned_head_moves_the_first_code_row_down_for_the_mouse_too() {
        // `draw_code` made this split itself and shadowed its own parameter, so every other reader
        // of the pane rectangle measured against one a row taller per pinned head. The mouse was
        // one of them: with three heads pinned a click landed three lines below the line under the
        // pointer, and since nothing is pinned at the top of a file it read as intermittent.
        let code = Rect::new(300.0, 120.0, 900.0, 600.0);
        const ROW: f32 = 26.0;

        let none = CodeArea::new(code, Vec::new(), ROW);
        assert_eq!(none.code, code, "nothing pinned changes nothing");
        assert!(none.pinned.is_none());

        for heads in 1..=3usize {
            let area = CodeArea::new(code, (0..heads).collect(), ROW);
            let strip = area.pinned.expect("a strip for the pinned heads");
            assert_eq!(strip.y, code.y);
            assert_eq!(strip.height, ROW * heads as f32);
            assert_eq!(area.code.y, code.y + ROW * heads as f32);
            assert_eq!(
                area.code.height,
                code.height - ROW * heads as f32,
                "the strip comes out of the scrolling rows, not out of nowhere"
            );

            // What the defect actually was: the first row of code, at the pixel it is drawn at,
            // must be display row zero.
            let grid = CodeGrid::new(area.code, 8.0, ROW, 0, 1.0);
            assert_eq!(grid.row_at(area.code.y + ROW / 2.0), 0);
            assert_eq!(grid.row_at(area.code.y + ROW * 1.5), 1);
        }
    }
}

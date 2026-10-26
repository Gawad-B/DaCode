//! The help panel: which keys do what, right now, without leaving the editor.
//!
//! `CHEATSHEET.md` has always had this, and being a file on disk it has always been read by nobody
//! who needed it — a person who cannot work out how to leave insert mode is not going to find a
//! markdown file in a repository. F1 is where every application in the world keeps this.
//!
//! **It shows the keys for the grammar you are actually using.** A non-modal user reading `dd` and
//! `ciw` learns nothing except that this editor is not for them, which was the exact complaint that
//! prompted all of this. The same table drives both, filtered by mode — see `dacode_input::commands`.
//!
//! Filterable, because a list of seventy keys is a reference and not an answer: typing "comment"
//! leaves the one line you wanted.

use dacode_input::commands::{self, Command, Group};
use dacode_render::Rect;

/// The panel's state.
#[derive(Debug, Clone)]
pub struct Help {
    /// Which grammar's keys are being shown.
    pub modal: bool,
    /// What has been typed to narrow the list.
    pub query: String,
    /// First visible row, for scrolling.
    pub scroll: usize,
    /// The highlighted line — always a binding, when there is one — which Enter runs.
    pub selected: usize,
}

/// Where the panel is, as data: drawing and the mouse both read it. See [`Help::geometry`].
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub card: Rect,
    /// The filter field across the top.
    pub input: Rect,
    /// One rectangle per visible line, headings included, top to bottom: `rows[i]` is line
    /// `first + i` of [`Help::lines`].
    pub rows: Vec<Rect>,
    /// The line the first of `rows` shows. The scroll position, clamped to the list.
    pub first: usize,
    /// How many lines the card has room for — what a page is, for scrolling.
    pub capacity: usize,
    /// The strip of key hints along the bottom.
    pub footer: Rect,
}

/// A line in the panel: either a heading or a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Heading(&'static str),
    Binding {
        /// The command it runs, by the name the palette and config know it by.
        name: &'static str,
        keys: &'static str,
        what: &'static str,
    },
}

impl Help {
    pub fn new(modal: bool) -> Self {
        Self {
            modal,
            query: String::new(),
            scroll: 0,
            // Line 0 is always a heading; its first binding is under it.
            selected: 1,
        }
    }

    /// The lines to draw.
    ///
    /// Commands with no key in this mode are left out. The panel answers "what can I press", and a
    /// row with an empty key column answers something else — the palette is where those live.
    pub fn lines(&self) -> Vec<Line> {
        let query = self.query.trim().to_lowercase();
        let mut lines = Vec::new();

        for group in Group::ALL {
            let matching: Vec<&Command> = commands::in_group(group)
                .filter(|command| !command.keys_for(self.modal).is_empty())
                .filter(|command| matches(command, &query, self.modal))
                .collect();

            // A heading with nothing under it is noise, and eight of them is a panel that looks
            // full while saying nothing.
            if matching.is_empty() {
                continue;
            }
            lines.push(Line::Heading(group.title()));
            for command in matching {
                lines.push(Line::Binding {
                    name: command.name,
                    keys: command.keys_for(self.modal),
                    what: command.what,
                });
            }
        }

        lines
    }

    /// Scrolls, clamped to the end of the list.
    pub fn scroll_by(&mut self, delta: isize, rows: usize) {
        let total = self.lines().len();
        let last = total.saturating_sub(rows);
        self.scroll = match delta {
            d if d < 0 => self.scroll.saturating_sub(d.unsigned_abs()),
            d => (self.scroll + d as usize).min(last),
        };
    }

    /// Moves the highlight to the next binding `delta` away, skipping headings, and scrolls just
    /// enough to keep it among the `rows` on screen.
    pub fn move_selection(&mut self, delta: isize, rows: usize) {
        let lines = self.lines();
        let bindings: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| matches!(line, Line::Binding { .. }))
            .map(|(index, _)| index)
            .collect();
        let Some(last) = bindings.len().checked_sub(1) else {
            return;
        };
        let at = bindings
            .iter()
            .position(|index| *index >= self.selected)
            .unwrap_or(last);
        let to = match delta {
            d if d < 0 => at.saturating_sub(d.unsigned_abs()),
            d => (at + d as usize).min(last),
        };
        self.selected = bindings[to];

        let rows = rows.max(1);
        if self.selected < self.scroll {
            // Its heading comes into view with it, when there is room for both: a binding shown
            // without the section it belongs to has lost half of what it says.
            let heading =
                self.selected > 0 && matches!(lines.get(self.selected - 1), Some(Line::Heading(_)));
            self.scroll = self.selected - usize::from(heading && rows > 1);
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    /// The command the highlighted line runs.
    pub fn selection(&self) -> Option<&'static str> {
        match self.lines().get(self.selected) {
            Some(Line::Binding { name, .. }) => Some(name),
            _ => None,
        }
    }

    /// The command on line `index` of [`Help::lines`], if it is a binding.
    pub fn command_on(&self, index: usize) -> Option<&'static str> {
        match self.lines().get(index) {
            Some(Line::Binding { name, .. }) => Some(name),
            _ => None,
        }
    }

    /// Where the card and its lines are, inside `bounds`.
    pub fn geometry(&self, bounds: Rect, scale: f32) -> Geometry {
        let px = |v: f32| v * scale;
        let (margin, pad, input_height, gap) = (px(16.0), px(6.0), px(36.0), px(6.0));
        let (row_height, footer_height) = (px(26.0), px(32.0));

        let width = px(720.0).min(bounds.width - margin * 2.0).max(0.0);
        // A centred dialog, inset from top and bottom alike, with less inset on a short window.
        let inset = px(80.0).min(bounds.height * 0.1);
        let fixed = pad * 2.0 + input_height + gap + footer_height;
        let height = (bounds.height - inset * 2.0).max(fixed + row_height);
        let card = Rect::new(
            bounds.x + (bounds.width - width) / 2.0,
            bounds.y + inset.min((bounds.height - height).max(0.0) / 2.0),
            width,
            height.min(bounds.height),
        );

        let input = Rect::new(
            card.x + pad,
            card.y + pad,
            card.width - pad * 2.0,
            input_height,
        );
        let footer = Rect::new(
            card.x + pad,
            card.bottom() - pad - footer_height,
            card.width - pad * 2.0,
            footer_height,
        );
        let list_top = input.bottom() + gap;
        let capacity = (((footer.y - list_top) / row_height).floor() as usize).max(1);

        let total = self.lines().len();
        let first = self.scroll.min(total.saturating_sub(1));
        let rows = (0..capacity.min(total - first.min(total)))
            .map(|index| {
                Rect::new(
                    input.x,
                    list_top + index as f32 * row_height,
                    input.width,
                    row_height,
                )
            })
            .collect();

        Geometry {
            card,
            input,
            rows,
            first,
            capacity,
            footer,
        }
    }

    /// Which line a point is over, by its index in [`Help::lines`].
    pub fn line_at(geometry: &Geometry, x: f32, y: f32) -> Option<usize> {
        geometry
            .rows
            .iter()
            .position(|row| crate::layout::contains(*row, x, y))
            .map(|at| geometry.first + at)
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.refiltered();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refiltered();
    }

    /// Back to the top of a list that has just changed, with its first binding highlighted.
    fn refiltered(&mut self) {
        self.scroll = 0;
        self.selected = self
            .lines()
            .iter()
            .position(|line| matches!(line, Line::Binding { .. }))
            .unwrap_or(0);
    }
}

fn matches(command: &Command, query: &str, modal: bool) -> bool {
    if query.is_empty() {
        return true;
    }
    // The keys are searchable too, which is the other direction people use this panel in: "what
    // does ctrl-d do again".
    command.what.to_lowercase().contains(query)
        || command.name.contains(query)
        || command.keys_for(modal).contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_shows_the_keys_for_the_grammar_in_use() {
        // The complaint this whole change came from: turning Vim off changed almost nothing visible.
        // The help panel is where that difference has to be most obvious, so a non-modal user must
        // not be shown `dd` and a modal one must not be shown `<c-s-k>`.
        let flat = Help::new(false).lines();
        let modal = Help::new(true).lines();

        assert!(flat.contains(&Line::Binding {
            name: "toggle_comment",
            keys: "<c-/>",
            what: "comment or uncomment these lines"
        }));
        assert!(
            !flat
                .iter()
                .any(|line| matches!(line, Line::Binding { keys, .. } if *keys == "dd")),
            "a non-modal user has no dd"
        );
        assert!(
            modal
                .iter()
                .any(|line| matches!(line, Line::Binding { keys, .. } if keys.contains("dd"))),
            "a modal user does"
        );
    }

    #[test]
    fn every_heading_has_something_under_it() {
        for modal in [true, false] {
            let lines = Help::new(modal).lines();
            for (index, line) in lines.iter().enumerate() {
                if matches!(line, Line::Heading(_)) {
                    assert!(
                        matches!(lines.get(index + 1), Some(Line::Binding { .. })),
                        "an empty heading in the {} list",
                        if modal { "modal" } else { "flat" }
                    );
                }
            }
        }
    }

    #[test]
    fn a_query_narrows_to_what_was_asked_for() {
        let mut help = Help::new(false);
        for c in "comment".chars() {
            help.type_char(c);
        }
        let lines = help.lines();
        assert!(
            lines.iter().all(
                |line| !matches!(line, Line::Binding { what, .. } if !what.contains("comment"))
            ),
            "the filter left something unrelated behind"
        );
        assert!(lines.len() >= 2, "and left the heading and the answer");
    }

    #[test]
    fn a_key_can_be_looked_up_by_what_you_pressed() {
        // The other way people use this: they pressed something by accident and want to know what
        // it was.
        let mut help = Help::new(false);
        for c in "<c-d>".chars() {
            help.type_char(c);
        }
        assert!(
            help.lines().iter().any(|line| {
                matches!(line, Line::Binding { what, .. } if what.contains("next copy"))
            }),
            "searching for a key should find what it does"
        );
    }

    #[test]
    fn scrolling_stops_at_the_end() {
        let mut help = Help::new(true);
        help.scroll_by(1000, 10);
        let total = help.lines().len();
        assert_eq!(help.scroll, total.saturating_sub(10));
        help.scroll_by(-1000, 10);
        assert_eq!(help.scroll, 0);
    }

    #[test]
    fn the_panel_and_its_rows_stay_inside_the_window_at_every_size() {
        let inside = |outer: Rect, inner: Rect| {
            inner.x >= outer.x - 0.01
                && inner.y >= outer.y - 0.01
                && inner.right() <= outer.right() + 0.01
                && inner.bottom() <= outer.bottom() + 0.01
        };
        for scale in [1.0, 2.0] {
            for (width, height) in [
                (640.0, 480.0),
                (800.0, 600.0),
                (1280.0, 720.0),
                (1600.0, 1000.0),
                (2560.0, 1440.0),
            ] {
                let bounds = Rect::new(0.0, 0.0, width * scale, height * scale);
                let mut help = Help::new(true);
                help.scroll = 7;
                let geometry = help.geometry(bounds, scale);
                let at = format!("{width}x{height} at {scale}x");
                assert!(inside(bounds, geometry.card), "card at {at}");
                assert!(inside(geometry.card, geometry.input), "input at {at}");
                assert!(inside(geometry.card, geometry.footer), "footer at {at}");
                assert!(!geometry.rows.is_empty(), "no rows at {at}");
                assert!(geometry.rows.len() <= geometry.capacity);
                for row in &geometry.rows {
                    assert!(inside(geometry.card, *row), "{row:?} at {at}");
                    assert!(row.y >= geometry.input.bottom() && row.bottom() <= geometry.footer.y);
                }
            }
        }
    }

    #[test]
    fn only_the_visible_lines_have_rectangles() {
        let mut help = Help::new(false);
        let total = help.lines().len();
        help.scroll = 5;
        let geometry = help.geometry(Rect::new(0.0, 0.0, 1600.0, 1000.0), 1.0);
        assert_eq!(geometry.first, 5);
        assert_eq!(geometry.rows.len(), geometry.capacity.min(total - 5));
        assert!(
            geometry.capacity < total,
            "the list is longer than a page here"
        );
    }

    #[test]
    fn the_highlight_skips_headings_and_stays_in_view() {
        let mut help = Help::new(false);
        assert!(
            help.selection().is_some(),
            "something is highlighted from the start"
        );
        for _ in 0..40 {
            help.move_selection(1, 10);
            assert!(help.selection().is_some(), "landed on a heading");
            assert!(
                help.selected >= help.scroll && help.selected < help.scroll + 10,
                "{} is off the page starting at {}",
                help.selected,
                help.scroll
            );
        }
        let low = help.selected;
        help.move_selection(-3, 10);
        assert!(help.selected < low);
        assert!(help.selected >= help.scroll);
    }

    #[test]
    fn filtering_highlights_the_first_answer() {
        // Type, press Enter: the shortest way from "what was that key" to having pressed it.
        let mut help = Help::new(false);
        for c in "every command".chars() {
            help.type_char(c);
        }
        assert_eq!(help.selection(), Some("command_palette"));
    }
}

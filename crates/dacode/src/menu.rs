//! The menu bar and the context menu.
//!
//! One type for both, because they are the same thing pointed at different places: a list of named
//! commands, one of which is highlighted, that runs an [`Action`] when chosen. The bar hangs from a
//! title in the top row; the context menu hangs from wherever the pointer was.
//!
//! Why an editor whose premise is keyboard speed has menus at all: they are the only part of the
//! interface a person can use without knowing anything. Someone who has just opened DaCode for the
//! first time does not know that `gd` goes to a definition, and cannot search a palette for a word
//! they have not learned — but they can read four words along the top of a window, and they will
//! right-click. Every entry shows its key, so using the menu twice teaches the shortcut and the
//! third time it is not needed. That is the whole design: the menu exists to make itself redundant.
//!
//! Both are drawn on `Layer::Modal`, and both take the keyboard while open — arrows move, enter
//! chooses, escape closes.

use dacode_input::{Action, Command, commands};
use dacode_render::Rect;
use dacode_theme::space;

/// A menu on screen.
#[derive(Debug, Clone)]
pub struct Menu {
    pub kind: Kind,
    /// The titles along the bar. Empty for a context menu.
    pub titles: Vec<&'static str>,
    /// Which title is open, and therefore which items are showing.
    pub open: usize,
    /// Whether the card is the overflow's list of hidden titles rather than a menu. `open` is then
    /// the first hidden title, which is where choosing nothing and walking right leads.
    pub listing: bool,
    /// The items of the open menu.
    pub items: Vec<Item>,
    /// Which item is highlighted.
    pub selected: usize,
    /// Where a context menu is anchored, in physical pixels.
    pub at: (f32, f32),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Along the top of the window, one list at a time.
    Bar,
    /// At the pointer.
    Context,
}

/// One row.
#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    /// The keys that do the same thing, shown dimmed on the right. Empty when there are none.
    pub keys: String,
    pub action: Action,
    /// For a row of the overflow's list: the menu choosing it opens, in place of running
    /// `action`.
    pub opens: Option<usize>,
}

impl Item {
    fn from_command(command: &Command, modal: bool) -> Self {
        Self {
            label: command.what.to_string(),
            keys: command.shown(modal),
            // Every entry comes from the command table, so a menu cannot offer something the editor
            // does not have — and cannot show the wrong key for it either.
            action: command.action().unwrap_or(Action::DismissInfo),
            opens: None,
        }
    }
}

/// The titles along the bar, left to right — VSCode's, because a menu bar is the one place in an
/// interface where being where people expect matters more than being clever. They are always on
/// the title bar, drawn by it; a menu opens under one.
pub const TITLES: [&str; 8] = [
    "File",
    "Edit",
    "Selection",
    "View",
    "Go",
    "Run",
    "Terminal",
    "Help",
];

/// What is in each menu, by command name, in the order of [`TITLES`].
///
/// Run and Terminal hold what exists today — copying the run command, since there is no terminal
/// to run it in — and fill in as those features land.
const MENUS: [&[&str]; 8] = [
    &[
        "new_file",
        "open_folder",
        "save",
        "save_as",
        "share",
        "close_buffer",
        "settings",
        "quit",
    ],
    &[
        "undo",
        "redo",
        "cut",
        "copy",
        "paste",
        "search",
        "replace",
        "search_project",
        "toggle_comment",
    ],
    &[
        "select_all",
        "select_line",
        "select_next_occurrence",
        "select_all_occurrences",
        "add_caret_above",
        "add_caret_below",
    ],
    &[
        "command_palette",
        "toggle_tree",
        "toggle_diagnostics",
        "notifications",
        "toggle_soft_wrap",
        "split_vertical",
        "split_horizontal",
        "increase_font_size",
        "decrease_font_size",
        "toggle_theme",
    ],
    &[
        "goto_definition",
        "find_references",
        "goto_symbol",
        "goto_symbol_in_project",
        "jump_back",
        "jump_forward",
        "next_diagnostic",
        "previous_diagnostic",
    ],
    &["run"],
    &["run"],
    &["help", "command_palette"],
];

/// What a right-click offers.
///
/// Two halves: what you do to the *text* you have — the clipboard verbs everyone reaches for first —
/// and what you ask about the *code* under the pointer. It was only the second half, which meant the
/// most common reason anybody right-clicks in an editor, cut and paste, was not there at all.
///
/// Long, deliberately. A context menu is where someone who does not know the shortcuts finds out
/// what the editor can do at this spot, and every row prints the key that would have done it.
const CONTEXT: [&str; 14] = [
    "cut",
    "copy",
    "paste",
    "select_all",
    "select_next_occurrence",
    "undo",
    "redo",
    "toggle_comment",
    "duplicate_line_down",
    "goto_definition",
    "find_references",
    "rename",
    "code_action",
    "format",
];

/// What a right-click **on the file tree** offers.
///
/// A different list from the text one, because a right-click is about the thing under the pointer
/// and the thing under the pointer is a file, not a symbol. Until this existed the tree was
/// strictly read-only — which contradicts the rule that a feature reachable only by a key is not
/// finished, for exactly the audience that rule was written for.
const TREE: [&str; 4] = ["new_file", "rename_file", "delete_file", "search_project"];

impl Menu {
    /// The context menu for a file-tree row.
    pub fn tree(at: (f32, f32), modal: bool) -> Self {
        Self {
            kind: Kind::Context,
            titles: Vec::new(),
            open: 0,
            listing: false,
            items: TREE
                .iter()
                .filter_map(|name| commands::by_name(name))
                .map(|command| Item::from_command(command, modal))
                .collect(),
            selected: 0,
            at,
        }
    }

    /// The bar, with its first menu open.
    ///
    /// Takes the grammar rather than assuming it: this hardcoded `true`, so with Vim off the file
    /// menu still advertised `:w  <c-s>` for save and `:q  <c-q>` for quit — two ex-commands whose
    /// leading `:` the user's own grammar cannot even parse.
    pub fn bar(modal: bool) -> Self {
        Self::bar_at(0, modal)
    }

    /// The bar as a click on a title leaves it: that title's menu, or for the overflow (`index`
    /// at or past `shown`, the number of titles on the bar) the list of the hidden ones.
    pub fn from_title(index: usize, shown: usize, modal: bool) -> Self {
        let mut menu = Self::bar_at(index.min(shown), modal);
        menu.choose_title(index, shown, modal);
        menu
    }

    /// The bar, with the menu under one title open — what F10 and the arrows reach.
    pub fn bar_at(open: usize, modal: bool) -> Self {
        let mut menu = Self {
            kind: Kind::Bar,
            titles: TITLES.to_vec(),
            open: open.min(TITLES.len() - 1),
            listing: false,
            items: Vec::new(),
            selected: 0,
            at: (0.0, 0.0),
        };
        menu.fill(modal);
        menu
    }

    /// The context menu, anchored at a point.
    pub fn context(at: (f32, f32), modal: bool) -> Self {
        Self {
            kind: Kind::Context,
            titles: Vec::new(),
            open: 0,
            listing: false,
            items: CONTEXT
                .iter()
                .filter_map(|name| commands::by_name(name))
                .map(|command| Item::from_command(command, modal))
                .collect(),
            selected: 0,
            at,
        }
    }

    /// Rebuilds the open menu's items.
    pub fn fill(&mut self, modal: bool) {
        if self.kind == Kind::Context {
            return;
        }
        if self.listing {
            return;
        }
        self.items = MENUS[self.open.min(MENUS.len() - 1)]
            .iter()
            .filter_map(|name| commands::by_name(name))
            .map(|command| Item::from_command(command, modal))
            .collect();
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    /// Moves along the bar. A no-op for a context menu, which has one list.
    pub fn move_menu(&mut self, delta: isize, modal: bool) {
        if self.kind == Kind::Context || self.titles.is_empty() {
            return;
        }
        let count = self.titles.len() as isize;
        // Wraps: a menu bar you cannot walk off the end of is one where the last menu is two
        // presses away from the first for no reason.
        self.open = (((self.open as isize + delta) % count + count) % count) as usize;
        self.listing = false;
        self.selected = 0;
        self.fill(modal);
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let count = self.items.len() as isize;
        self.selected = (((self.selected as isize + delta) % count + count) % count) as usize;
    }

    pub fn selection(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    /// Where the menu is, as data.
    ///
    /// One definition, read by the drawing code and by the mouse. Two copies of this arithmetic
    /// drift, and the symptom — clicking one row and getting its neighbour — gets blamed on
    /// everything except the copy that drifted. The same rule `layout.rs` exists for.
    ///
    /// A menu that could only be driven from the keyboard would also be a joke: the menu bar is
    /// *for* the person reaching for a mouse.
    pub fn geometry(
        &self,
        bounds: Rect,
        scale: f32,
        typography: &dacode_theme::Typography,
    ) -> Geometry {
        let row_height = 24.0 * scale;
        let pad = space::MD * scale;

        // Text is measured at the same nominal advance the chrome draws menu text with. Exact
        // shaping would need the font; this is a menu, and being a few pixels wide is invisible.
        let label_width = self.widest(|item| item.label.chars().count()) * 8.0 * scale;
        let keys_width = self.widest(|item| item.keys.chars().count()) * 8.0 * scale;
        let width = (label_width + keys_width + pad * 6.0).min(bounds.width - pad * 2.0);

        let (mut titles, mut overflow, mut header) = (Vec::new(), None, false);
        if self.kind == Kind::Bar {
            let bar = crate::layout::title_bar(
                crate::layout::title_strip(bounds, scale),
                scale,
                typography,
            );
            titles = bar.menus;
            overflow = bar.overflow;
            // A menu behind the overflow hangs under "…", which does not say which menu it is.
            header = !self.listing && self.open >= titles.len();
        }
        let header_height = if header { row_height } else { 0.0 };
        let height = self.items.len() as f32 * row_height + header_height + pad * 2.0;

        let (x, y) = match self.kind {
            // Under its title in the title bar — read from the title bar's own geometry, which is
            // what draws the titles, so the card cannot hang under a word that is somewhere else.
            Kind::Bar => {
                let title = crate::layout::title_strip(bounds, scale);
                let anchor = titles
                    .get(self.open)
                    .copied()
                    .or(overflow)
                    .unwrap_or(Rect::new(title.x, title.y, 0.0, title.height));
                (
                    anchor.x.min(bounds.right() - width).max(bounds.x),
                    anchor.bottom() + 4.0 * scale,
                )
            }
            // Clamped, so a right-click near an edge still shows the whole menu rather than half of
            // it off the window.
            Kind::Context => (
                self.at.0.min(bounds.right() - width - pad).max(bounds.x),
                self.at.1.min(bounds.bottom() - height - pad).max(bounds.y),
            ),
        };

        let card = Rect::new(x, y, width, height);
        let header = header.then(|| Rect::new(card.x, card.y + pad, card.width, row_height));
        let rows = (0..self.items.len())
            .map(|index| {
                Rect::new(
                    card.x,
                    card.y + pad + header_height + index as f32 * row_height,
                    card.width,
                    row_height,
                )
            })
            .collect();

        Geometry {
            titles,
            overflow,
            header,
            card,
            rows,
            label_width,
            pad,
        }
    }

    fn widest(&self, measure: impl Fn(&Item) -> usize) -> f32 {
        self.items.iter().map(measure).max().unwrap_or(10) as f32
    }

    /// Which row a point is over, if any.
    pub fn row_at(&self, geometry: &Geometry, x: f32, y: f32) -> Option<usize> {
        geometry
            .rows
            .iter()
            .position(|row| crate::layout::contains(*row, x, y))
    }

    /// Which title is under a point, if any: its index in [`TITLES`], or for the overflow the
    /// number of titles on the bar — the first that did not fit. See [`choose_title`](Self::choose_title).
    pub fn title_at(&self, geometry: &Geometry, x: f32, y: f32) -> Option<usize> {
        geometry
            .titles
            .iter()
            .position(|title| crate::layout::contains(*title, x, y))
            .or_else(|| {
                geometry
                    .overflow
                    .filter(|rect| crate::layout::contains(*rect, x, y))
                    .map(|_| geometry.titles.len())
            })
    }

    /// The title the open menu hangs from: its own, or the overflow for a menu behind it.
    pub fn open_title(&self, geometry: &Geometry) -> Option<Rect> {
        if self.kind == Kind::Context {
            return None;
        }
        geometry
            .titles
            .get(self.open)
            .copied()
            .or(geometry.overflow)
    }

    /// Whether pressing the title [`title_at`](Self::title_at) answered with closes the card
    /// rather than changing it. For the overflow, only while its list is showing: with a hidden
    /// menu open under it, pressing "…" goes back to the list, so every hidden menu stays one
    /// click from every other.
    pub fn is_open_title(&self, geometry: &Geometry, index: usize) -> bool {
        if index >= geometry.titles.len() {
            self.listing
        } else {
            index == self.open && !self.listing
        }
    }

    /// Opens the menu under a title, from its first row.
    pub fn open_menu(&mut self, index: usize, modal: bool) {
        if self.kind == Kind::Context {
            return;
        }
        self.open = index.min(TITLES.len() - 1);
        self.listing = false;
        self.selected = 0;
        self.fill(modal);
    }

    /// What a press on a title does: a title on the bar opens its menu, and the overflow lists
    /// the titles that did not fit — `shown` is how many did.
    pub fn choose_title(&mut self, index: usize, shown: usize, modal: bool) {
        if self.kind == Kind::Context {
            return;
        }
        if index < shown {
            self.open_menu(index, modal);
            return;
        }
        let first = shown.min(TITLES.len() - 1);
        self.open = first;
        self.listing = true;
        self.selected = 0;
        self.items = TITLES[first..]
            .iter()
            .enumerate()
            .map(|(offset, title)| Item {
                label: title.to_string(),
                keys: String::new(),
                action: Action::DismissInfo,
                opens: Some(first + offset),
            })
            .collect();
    }

    /// Follows a row of the overflow's list into its menu. Returns whether the row was one; any
    /// other row is left to the caller to run.
    pub fn follow(&mut self, row: usize, modal: bool) -> bool {
        match self.items.get(row).and_then(|item| item.opens) {
            Some(index) => {
                self.open_menu(index, modal);
                true
            }
            None => false,
        }
    }

    /// The pointer moved while the bar is open: over another title, that menu opens, as VSCode's
    /// bar does once one is open. Returns whether anything changed. Over the open title nothing
    /// does, so the highlighted row is not reset under a pointer that merely wobbled.
    pub fn hover_title(&mut self, geometry: &Geometry, x: f32, y: f32, modal: bool) -> bool {
        if self.kind == Kind::Context {
            return false;
        }
        let shown = geometry.titles.len();
        match self.title_at(geometry, x, y) {
            // Over "…" with one of its menus open, nothing changes: that menu hangs under it.
            Some(index) if index >= shown && !self.listing && self.open >= shown => false,
            Some(index) if !self.is_open_title(geometry, index) => {
                self.choose_title(index, shown, modal);
                true
            }
            _ => false,
        }
    }
}

/// Where a menu's parts are.
#[derive(Debug, Clone)]
pub struct Geometry {
    /// The bar's titles that fit, left to right — [`crate::layout::TitleBar::menus`]. Empty for a
    /// context menu.
    pub titles: Vec<Rect>,
    /// The title standing for those that did not fit.
    pub overflow: Option<Rect>,
    /// A row naming the open menu, above its items, when its own title is not on the bar.
    pub header: Option<Rect>,
    /// The open list.
    pub card: Rect,
    /// One rectangle per item, in order.
    pub rows: Vec<Rect>,
    /// Width of the label column, so the drawing code does not measure twice.
    pub label_width: f32,
    pub pad: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typography() -> dacode_theme::Typography {
        dacode_theme::Typography::default()
    }

    #[test]
    fn every_menu_entry_names_a_real_command() {
        // A menu is a promise about what the editor can do. An entry naming a command that does not
        // exist would silently vanish from the menu instead of failing loudly, and nobody would
        // notice until a user went looking for it.
        for (title, names) in TITLES.iter().zip(MENUS) {
            for name in names {
                assert!(
                    commands::by_name(name).is_some(),
                    "the {title} menu offers `{name}`, which is not a command"
                );
            }
        }
        for name in CONTEXT.iter().chain(TREE.iter()) {
            assert!(
                commands::by_name(name).is_some(),
                "a context menu offers `{name}`, which is not a command"
            );
        }
    }

    #[test]
    fn every_menu_entry_resolves_to_an_action_rather_than_falling_back() {
        // `Item::from_command` falls back to `DismissInfo` when a name resolves to no action, which
        // closes the menu and does nothing — from the outside, indistinguishable from a menu that
        // cannot be clicked at all. The test above only checks the name is in the command table, and
        // a name can be in the table with nothing behind it.
        let named = CONTEXT
            .iter()
            .chain(TREE.iter())
            .chain(MENUS.iter().flat_map(|names| names.iter()));
        for name in named {
            let command = commands::by_name(name).expect("checked above");
            assert!(
                command.action().is_some(),
                "`{name}` is offered in a menu but resolves to no action, so choosing it does nothing"
            );
        }
    }

    #[test]
    fn the_bar_opens_on_its_first_menu_with_items_in_it() {
        let menu = Menu::bar(true);
        assert_eq!(menu.titles, TITLES.to_vec());
        assert_eq!(menu.titles.first(), Some(&"File"));
        assert!(!menu.items.is_empty());
        assert_eq!(
            menu.selection().map(|item| item.action.clone()),
            Some(Action::NewFile)
        );
    }

    #[test]
    fn the_file_menu_is_where_the_share_button_went() {
        // The title bar's Share button is gone; copying a link to the line is a File entry now,
        // and a command nobody can reach from the mouse is not finished.
        let menu = Menu::bar(false);
        assert!(
            menu.items.iter().any(|item| item.action == Action::Share),
            "File offers share: {:?}",
            menu.items
                .iter()
                .map(|item| &item.label)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_run_entries_say_what_they_really_do() {
        // There is no terminal yet, and `run` copies the command for one. An entry reading "run the
        // project", under Run or Terminal, would promise something that does not happen.
        for title in ["Run", "Terminal"] {
            let menu = Menu::bar_at(TITLES.iter().position(|t| *t == title).unwrap(), false);
            let run = menu
                .items
                .iter()
                .find(|item| item.action == Action::Run)
                .unwrap_or_else(|| panic!("{title} offers run"));
            assert_eq!(run.label, "copy the run command", "under {title}");
        }
    }

    #[test]
    fn walking_off_the_end_of_the_bar_wraps() {
        let mut menu = Menu::bar(true);
        menu.move_menu(-1, true);
        assert_eq!(
            menu.open,
            TITLES.len() - 1,
            "left from the first is the last"
        );
        menu.move_menu(1, true);
        assert_eq!(menu.open, 0);
    }

    #[test]
    fn a_menu_entry_carries_the_key_that_does_the_same_thing() {
        // The point of the menu: you read it once and learn the shortcut. An entry with no key is
        // allowed — some commands have none — but save is not one of them.
        let menu = Menu::bar(true);
        let save = menu
            .items
            .iter()
            .find(|item| item.action == Action::Save)
            .expect("file menu has save");
        assert!(
            save.keys.contains("<c-s>"),
            "save should advertise its key, got {:?}",
            save.keys
        );
    }

    #[test]
    fn a_click_lands_on_the_row_it_looks_like_it_lands_on() {
        // Drawing and hit-testing read the same geometry; this is the test that says so. The bug it
        // prevents — clicking one row and getting its neighbour — is the classic symptom of two
        // copies of the arithmetic, and it always gets blamed on something else.
        let menu = Menu::bar(true);
        let bounds = Rect::new(0.0, 0.0, 1280.0, 820.0);
        let geometry = menu.geometry(bounds, 1.0, &typography());

        for (index, row) in geometry.rows.iter().enumerate() {
            let middle = (row.x + row.width / 2.0, row.y + row.height / 2.0);
            assert_eq!(
                menu.row_at(&geometry, middle.0, middle.1),
                Some(index),
                "the middle of row {index} is not row {index}"
            );
        }

        assert_eq!(
            menu.row_at(&geometry, geometry.card.x - 10.0, geometry.card.y),
            None,
            "a click beside the card is not a click on a row"
        );
    }

    #[test]
    fn a_context_menu_row_is_hit_at_a_fractional_scale_too() {
        // Every other menu test runs at scale 1.0, and the machine this was reported on does not.
        // This pins the half of the defect that was *not* broken, so a fix does not get aimed at
        // the geometry when the causes were hover, drag and the pad strip.
        let menu = Menu::context((640.0, 380.0), false);
        let bounds = Rect::new(0.0, 0.0, 1920.0, 1200.0);
        let geometry = menu.geometry(bounds, 1.5, &typography());

        for (index, row) in geometry.rows.iter().enumerate() {
            let middle = (row.x + row.width / 2.0, row.y + row.height / 2.0);
            assert_eq!(menu.row_at(&geometry, middle.0, middle.1), Some(index));
        }
    }

    #[test]
    fn the_card_has_a_padded_strip_that_is_not_a_row() {
        // The pointer sits on the card's top-left corner the moment a context menu opens: inside
        // the card, above the first row. The click handler treated "in the card but not on a row"
        // as clicking away, so right-click then left-click without moving dismissed the menu — the
        // first thing anyone tries. The geometry is right; the caller has to know about this strip.
        let menu = Menu::context((100.0, 200.0), false);
        let bounds = Rect::new(0.0, 0.0, 1280.0, 820.0);
        let geometry = menu.geometry(bounds, 1.0, &typography());

        let corner = (geometry.card.x + 2.0, geometry.card.y + 2.0);
        assert_eq!(menu.row_at(&geometry, corner.0, corner.1), None);
        assert!(
            crate::layout::contains(geometry.card, corner.0, corner.1),
            "the corner is inside the card, which is why dismissing on it was wrong"
        );
    }

    #[test]
    fn clicking_a_title_is_how_the_bar_is_walked_with_a_mouse() {
        let menu = Menu::bar(true);
        let bounds = Rect::new(0.0, 0.0, 1280.0, 820.0);
        let geometry = menu.geometry(bounds, 1.0, &typography());
        let second = geometry.titles[1];

        assert_eq!(
            menu.title_at(&geometry, second.x + 2.0, second.y + 2.0),
            Some(1)
        );
    }

    #[test]
    fn the_bar_reads_its_titles_from_the_title_bar_and_hangs_the_card_under_the_open_one() {
        // The titles are drawn by the title bar, always; the menu only opens under one. Two copies
        // of where a title is would be a dropdown hanging under the wrong word.
        let bounds = Rect::new(0.0, 0.0, 1600.0, 1000.0);
        let title = Rect::new(0.0, 0.0, 1600.0, crate::layout::size::TITLE);
        let bar = crate::layout::title_bar(title, 1.0, &typography());
        for (index, name) in TITLES.iter().enumerate() {
            let menu = Menu::bar_at(index, false);
            let geometry = menu.geometry(bounds, 1.0, &typography());
            assert_eq!(geometry.titles, bar.menus);
            let open = geometry.titles[index];
            assert_eq!(geometry.card.x, open.x, "{name} hangs under its title");
            assert!(geometry.card.y >= open.bottom());
        }
    }

    #[test]
    fn the_overflow_lists_the_hidden_menus_and_each_opens_under_it_with_its_name() {
        // At a narrow width the trailing menus are behind "…". It used to open only the first of
        // them, and pressing it again closed that — so Help, at the end, could not be reached by a
        // mouse at all. It lists them now, and the card of a hidden menu says which menu it is,
        // because the "…" it hangs from does not.
        let bounds = Rect::new(0.0, 0.0, 640.0, 480.0);
        let probe = Menu::bar(false).geometry(bounds, 1.0, &typography());
        let overflow = probe.overflow.expect("640px cannot hold eight titles");
        let shown = probe.titles.len();
        assert_eq!(
            Menu::bar(false).title_at(&probe, overflow.x + 2.0, overflow.y + 2.0),
            Some(shown)
        );

        let mut menu = Menu::from_title(shown, shown, false);
        assert!(menu.listing);
        let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(
            labels,
            TITLES[shown..].to_vec(),
            "every hidden title, in order"
        );
        let geometry = menu.geometry(bounds, 1.0, &typography());
        assert_eq!(menu.open_title(&geometry), geometry.overflow);
        assert!(geometry.header.is_none(), "the list is not a menu to name");
        assert!(
            menu.is_open_title(&geometry, shown),
            "pressing \"…\" again closes its list"
        );

        let help = labels.iter().position(|label| *label == "Help").unwrap();
        assert!(menu.follow(help, false));
        assert_eq!(TITLES[menu.open], "Help");
        assert!(
            menu.items
                .iter()
                .any(|item| item.action == Action::ToggleHelp)
        );
        let geometry = menu.geometry(bounds, 1.0, &typography());
        assert_eq!(menu.open_title(&geometry), geometry.overflow);
        let header = geometry.header.expect("a hidden menu is named");
        assert!(header.bottom() <= geometry.rows[0].y, "above the first row");
        assert!(geometry.card.right() <= bounds.right(), "on the window");
        assert!(
            !menu.is_open_title(&geometry, shown),
            "pressing \"…\" over a hidden menu goes back to the list"
        );
    }

    #[test]
    fn moving_over_another_title_while_one_is_open_opens_that_one() {
        // VSCode's bar: once a menu is open, sweeping along the titles opens each in turn. Moving
        // over the open one changes nothing, so the highlighted row is not reset under the pointer.
        let bounds = Rect::new(0.0, 0.0, 1600.0, 1000.0);
        let mut menu = Menu::bar(false);
        menu.selected = 2;
        let geometry = menu.geometry(bounds, 1.0, &typography());
        let file = geometry.titles[0];
        assert!(!menu.hover_title(&geometry, file.x + 2.0, file.y + 2.0, false));
        assert_eq!(menu.selected, 2);

        let edit = geometry.titles[1];
        assert!(menu.hover_title(&geometry, edit.x + 2.0, edit.y + 2.0, false));
        assert_eq!(menu.open, 1);
        assert_eq!(menu.selected, 0);
        assert!(menu.items.iter().any(|item| item.action == Action::Undo));
    }

    #[test]
    fn a_right_click_on_the_tree_offers_file_operations_not_symbol_ones() {
        // The tree was strictly read-only, which contradicts this project's rule that a feature
        // reachable only by a key is not finished — for exactly the audience the mouse layer was
        // built for. A right-click is about what is under the pointer, and on the tree that is a
        // file, so it cannot be the same list as the text one.
        let tree = Menu::tree((100.0, 200.0), false);
        let names: Vec<&str> = tree.items.iter().map(|item| item.label.as_str()).collect();

        assert!(
            tree.items
                .iter()
                .any(|item| item.action == Action::DeleteFile),
            "the tree menu must offer the file operations: {names:?}"
        );
        assert!(
            !tree
                .items
                .iter()
                .any(|item| item.action == Action::GotoDefinition),
            "and not the ones about a symbol: {names:?}"
        );
    }

    #[test]
    fn a_context_menu_at_the_edge_is_pushed_back_onto_the_window() {
        // Right-clicking near the bottom right is normal, and a menu that hung off the window there
        // would be unusable exactly where people click most.
        let menu = Menu::context((1275.0, 815.0), false);
        let bounds = Rect::new(0.0, 0.0, 1280.0, 820.0);
        let geometry = menu.geometry(bounds, 1.0, &typography());

        assert!(
            geometry.card.right() <= bounds.right(),
            "off the right edge"
        );
        assert!(geometry.card.bottom() <= bounds.bottom(), "off the bottom");
    }

    #[test]
    fn the_context_menu_is_about_the_code_under_the_pointer() {
        let menu = Menu::context((100.0, 200.0), false);
        assert_eq!(menu.kind, Kind::Context);
        assert_eq!(menu.at, (100.0, 200.0));
        assert!(
            menu.items
                .iter()
                .any(|item| item.action == Action::GotoDefinition),
            "a right-click has to offer the thing people right-click for"
        );
    }
}

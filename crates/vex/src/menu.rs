//! The menu bar and the context menu.
//!
//! One type for both, because they are the same thing pointed at different places: a list of named
//! commands, one of which is highlighted, that runs an [`Action`] when chosen. The bar hangs from a
//! title in the top row; the context menu hangs from wherever the pointer was.
//!
//! Why an editor whose premise is keyboard speed has menus at all: they are the only part of the
//! interface a person can use without knowing anything. Someone who has just opened vex for the
//! first time does not know that `gd` goes to a definition, and cannot search a palette for a word
//! they have not learned — but they can read four words along the top of a window, and they will
//! right-click. Every entry shows its key, so using the menu twice teaches the shortcut and the
//! third time it is not needed. That is the whole design: the menu exists to make itself redundant.
//!
//! Both are drawn on `Layer::Modal`, and both take the keyboard while open — arrows move, enter
//! chooses, escape closes.

use vex_input::{Action, Command, commands};
use vex_render::Rect;
use vex_theme::space;

/// A menu on screen.
#[derive(Debug, Clone)]
pub struct Menu {
    pub kind: Kind,
    /// The titles along the bar. Empty for a context menu.
    pub titles: Vec<&'static str>,
    /// Which title is open, and therefore which items are showing.
    pub open: usize,
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
}

impl Item {
    fn from_command(command: &Command, modal: bool) -> Self {
        Self {
            label: command.what.to_string(),
            keys: command.keys_for(modal).to_string(),
            // Every entry comes from the command table, so a menu cannot offer something the editor
            // does not have — and cannot show the wrong key for it either.
            action: command.action().unwrap_or(Action::DismissInfo),
        }
    }
}

/// The menus along the bar, and what is in each.
///
/// Named by what they are for rather than by the industry's habit — "go" and "code" say more than
/// "edit" and "tools" — but File and Edit keep their names, because those two are muscle memory.
const MENUS: [(&str, &[&str]); 5] = [
    (
        "file",
        &[
            "save",
            "save_as",
            "open_folder",
            "close_buffer",
            "toggle_tree",
            "settings",
            "quit",
        ],
    ),
    (
        "edit",
        &[
            "undo",
            "redo",
            "paste",
            "replace",
            "toggle_comment",
            "duplicate_line_down",
            "delete_line",
            "format",
        ],
    ),
    (
        "selection",
        &[
            "select_all",
            "select_line",
            "select_next_occurrence",
            "select_all_occurrences",
            "add_caret_below",
            "collapse_selections",
        ],
    ),
    (
        "go",
        &[
            "goto_definition",
            "find_references",
            "search",
            "search_project",
            "next_diagnostic",
            "jump_back",
        ],
    ),
    (
        "help",
        &["help", "command_palette", "toggle_diagnostics", "hover"],
    ),
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
        let mut menu = Self {
            kind: Kind::Bar,
            titles: MENUS.iter().map(|(title, _)| *title).collect(),
            open: 0,
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
        let (_, names) = MENUS[self.open.min(MENUS.len() - 1)];
        self.items = names
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
    pub fn geometry(&self, bounds: Rect, scale: f32) -> Geometry {
        let row_height = 24.0 * scale;
        let pad = space::MD * scale;

        // Text is measured at the same nominal advance the chrome draws menu text with. Exact
        // shaping would need the font; this is a menu, and being a few pixels wide is invisible.
        let label_width = self.widest(|item| item.label.chars().count()) * 8.0 * scale;
        let keys_width = self.widest(|item| item.keys.chars().count()) * 8.0 * scale;
        let width = (label_width + keys_width + pad * 6.0).min(bounds.width - pad * 2.0);
        let height = self.items.len() as f32 * row_height + pad * 2.0;

        let mut titles = Vec::new();
        let (x, y) = match self.kind {
            Kind::Bar => {
                let title_y = bounds.y + 40.0 * scale;
                let mut x = bounds.x + space::LG * scale;
                let mut open_x = x;
                for (index, title) in self.titles.iter().enumerate() {
                    let width = (title.chars().count() as f32 + 2.0) * 9.0 * scale;
                    titles.push(Rect::new(x, title_y, width, row_height));
                    if index == self.open {
                        open_x = x;
                    }
                    x += width;
                }
                (open_x, title_y + row_height)
            }
            // Clamped, so a right-click near an edge still shows the whole menu rather than half of
            // it off the window.
            Kind::Context => (
                self.at.0.min(bounds.right() - width - pad).max(bounds.x),
                self.at.1.min(bounds.bottom() - height - pad).max(bounds.y),
            ),
        };

        let card = Rect::new(x, y, width, height);
        let rows = (0..self.items.len())
            .map(|index| {
                Rect::new(
                    card.x,
                    card.y + pad + index as f32 * row_height,
                    card.width,
                    row_height,
                )
            })
            .collect();

        Geometry {
            titles,
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

    /// Which title along the bar a point is over, if any.
    pub fn title_at(&self, geometry: &Geometry, x: f32, y: f32) -> Option<usize> {
        geometry
            .titles
            .iter()
            .position(|title| crate::layout::contains(*title, x, y))
    }
}

/// Where a menu's parts are.
#[derive(Debug, Clone)]
pub struct Geometry {
    /// The bar's titles, left to right. Empty for a context menu.
    pub titles: Vec<Rect>,
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

    #[test]
    fn every_menu_entry_names_a_real_command() {
        // A menu is a promise about what the editor can do. An entry naming a command that does not
        // exist would silently vanish from the menu instead of failing loudly, and nobody would
        // notice until a user went looking for it.
        for (title, names) in MENUS {
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
            .chain(MENUS.iter().flat_map(|(_, names)| names.iter()));
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
        assert_eq!(menu.titles.first(), Some(&"file"));
        assert!(!menu.items.is_empty());
        assert_eq!(
            menu.selection().map(|item| item.action.clone()),
            Some(Action::Save)
        );
    }

    #[test]
    fn walking_off_the_end_of_the_bar_wraps() {
        let mut menu = Menu::bar(true);
        menu.move_menu(-1, true);
        assert_eq!(
            menu.open,
            MENUS.len() - 1,
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
        let save = menu.selection().expect("file menu has entries");
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
        let geometry = menu.geometry(bounds, 1.0);

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
        let geometry = menu.geometry(bounds, 1.5);

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
        let geometry = menu.geometry(bounds, 1.0);

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
        let geometry = menu.geometry(bounds, 1.0);
        let second = geometry.titles[1];

        assert_eq!(
            menu.title_at(&geometry, second.x + 2.0, second.y + 2.0),
            Some(1)
        );
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
        let geometry = menu.geometry(bounds, 1.0);

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

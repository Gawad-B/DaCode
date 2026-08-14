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
        &["save", "open_folder", "toggle_tree", "settings", "quit"],
    ),
    (
        "edit",
        &[
            "undo",
            "redo",
            "paste",
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

/// What a right-click offers: the things that are about the thing under the pointer.
const CONTEXT: [&str; 8] = [
    "goto_definition",
    "find_references",
    "rename",
    "code_action",
    "select_next_occurrence",
    "paste",
    "toggle_comment",
    "format",
];

impl Menu {
    /// The bar, with its first menu open.
    pub fn bar() -> Self {
        let mut menu = Self {
            kind: Kind::Bar,
            titles: MENUS.iter().map(|(title, _)| *title).collect(),
            open: 0,
            items: Vec::new(),
            selected: 0,
            at: (0.0, 0.0),
        };
        menu.fill(true);
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
        for name in CONTEXT {
            assert!(
                commands::by_name(name).is_some(),
                "the context menu offers `{name}`, which is not a command"
            );
        }
    }

    #[test]
    fn the_bar_opens_on_its_first_menu_with_items_in_it() {
        let menu = Menu::bar();
        assert_eq!(menu.titles.first(), Some(&"file"));
        assert!(!menu.items.is_empty());
        assert_eq!(menu.selection().map(|item| item.action.clone()), Some(Action::Save));
    }

    #[test]
    fn walking_off_the_end_of_the_bar_wraps() {
        let mut menu = Menu::bar();
        menu.move_menu(-1, true);
        assert_eq!(menu.open, MENUS.len() - 1, "left from the first is the last");
        menu.move_menu(1, true);
        assert_eq!(menu.open, 0);
    }

    #[test]
    fn a_menu_entry_carries_the_key_that_does_the_same_thing() {
        // The point of the menu: you read it once and learn the shortcut. An entry with no key is
        // allowed — some commands have none — but save is not one of them.
        let menu = Menu::bar();
        let save = menu.selection().expect("file menu has entries");
        assert!(
            save.keys.contains("<c-s>"),
            "save should advertise its key, got {:?}",
            save.keys
        );
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

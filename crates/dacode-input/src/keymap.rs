//! User key bindings, layered over the built-in grammar.
//!
//! A keymap is consulted *before* the grammar, and only when the grammar is idle. That restriction
//! is the whole design: if a binding could fire while an operator is pending, `d<leader>x` would
//! either eat the operator or leave it dangling, and the editor's most important invariant — that a
//! half-typed command is always either completable or cancellable — would depend on what the user
//! put in their config.
//!
//! Bindings are sequences, not single keys, because the useful ones are (`<leader>ff`). A sequence
//! that starts matching and then fails is *replayed* through the grammar rather than swallowed:
//! typing `gd` when only `gx` is bound must still reach the built-in `gd`.

use crate::action::Action;
use crate::key::Key;
use dacode_theme::Mode;

/// The result of looking a key sequence up.
#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    /// No binding starts with these keys.
    None,
    /// A longer binding starts with these keys; wait for more.
    Prefix,
    Exact(Action),
}

#[derive(Debug, Clone)]
struct Entry {
    mode: Mode,
    keys: Vec<Key>,
    action: Action,
}

/// Why a binding was rejected. Reported to the user rather than ignored: a config error the editor
/// swallows is a config error the user cannot find.
#[derive(Debug, Clone, PartialEq)]
pub enum BindError {
    UnknownMode(String),
    UnknownAction(String),
    EmptySequence,
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::UnknownMode(mode) => write!(
                f,
                "unknown mode `{mode}` (expected normal, insert, visual or replace)"
            ),
            BindError::UnknownAction(action) => write!(f, "unknown action `{action}`"),
            BindError::EmptySequence => write!(f, "empty key sequence"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    entries: Vec<Entry>,
    /// What `<leader>` expands to. Space, as in every modern Vim configuration.
    leader: String,
}

impl Keymap {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            leader: "<space>".to_string(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Sets what `<leader>` means. Affects bindings added afterwards, so config sets it first.
    pub fn set_leader(&mut self, leader: &str) {
        self.leader = leader.to_string();
    }

    /// Binds a key sequence in a mode. Later bindings replace earlier ones for the same sequence,
    /// so a config that sets the same key twice behaves like assignment rather than accumulating a
    /// binding that can never fire.
    pub fn bind(&mut self, mode: &str, sequence: &str, action: &str) -> Result<(), BindError> {
        let mode = parse_mode(mode).ok_or_else(|| BindError::UnknownMode(mode.to_string()))?;
        let action = Action::from_name(action)
            .ok_or_else(|| BindError::UnknownAction(action.to_string()))?;

        let keys = Key::parse_sequence(&sequence.replace("<leader>", &self.leader));
        if keys.is_empty() {
            return Err(BindError::EmptySequence);
        }

        self.entries
            .retain(|entry| !(entry.mode == mode && entry.keys == keys));
        self.entries.push(Entry { mode, keys, action });
        Ok(())
    }

    /// Looks up a partial sequence. `Prefix` means keep collecting.
    pub fn lookup(&self, mode: Mode, keys: &[Key]) -> Match {
        if keys.is_empty() {
            return Match::None;
        }

        let mut prefix = false;
        for entry in &self.entries {
            if entry.mode != mode || !entry.keys.starts_with(keys) {
                continue;
            }
            if entry.keys.len() == keys.len() {
                return Match::Exact(entry.action.clone());
            }
            prefix = true;
        }

        if prefix { Match::Prefix } else { Match::None }
    }
}

fn parse_mode(name: &str) -> Option<Mode> {
    match name.to_ascii_lowercase().as_str() {
        "normal" | "n" => Some(Mode::Normal),
        "insert" | "i" => Some(Mode::Insert),
        "visual" | "v" => Some(Mode::Visual),
        "replace" | "r" => Some(Mode::Replace),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(sequence: &str) -> Vec<Key> {
        Key::parse_sequence(sequence)
    }

    #[test]
    fn a_binding_resolves_to_its_action() {
        let mut map = Keymap::new();
        map.bind("normal", "<leader>w", "save").unwrap();

        // `<leader>` expanded to space, so the first key alone is only a prefix.
        assert_eq!(map.lookup(Mode::Normal, &keys("<space>")), Match::Prefix);
        assert_eq!(
            map.lookup(Mode::Normal, &keys("<space>w")),
            Match::Exact(Action::Save)
        );
    }

    #[test]
    fn bindings_are_per_mode() {
        let mut map = Keymap::new();
        map.bind("visual", "s", "split_selection_into_lines")
            .unwrap();

        assert_eq!(
            map.lookup(Mode::Visual, &keys("s")),
            Match::Exact(Action::SplitSelectionIntoLines)
        );
        assert_eq!(
            map.lookup(Mode::Normal, &keys("s")),
            Match::None,
            "a visual binding must not shadow a normal-mode key"
        );
    }

    #[test]
    fn rebinding_replaces_rather_than_accumulating() {
        let mut map = Keymap::new();
        map.bind("normal", "<leader>w", "save").unwrap();
        map.bind("normal", "<leader>w", "quit").unwrap();

        assert_eq!(map.len(), 1);
        assert_eq!(
            map.lookup(Mode::Normal, &keys("<space>w")),
            Match::Exact(Action::Quit),
            "the last binding wins"
        );
    }

    #[test]
    fn the_leader_is_configurable_and_applies_to_later_bindings() {
        let mut map = Keymap::new();
        map.set_leader(",");
        map.bind("normal", "<leader>f", "search_project").unwrap();

        assert_eq!(
            map.lookup(Mode::Normal, &keys(",f")),
            Match::Exact(Action::SearchProject)
        );
        assert_eq!(map.lookup(Mode::Normal, &keys("<space>f")), Match::None);
    }

    #[test]
    fn a_typo_is_an_error_rather_than_a_dead_key() {
        let mut map = Keymap::new();
        assert_eq!(
            map.bind("normal", "<leader>x", "sav"),
            Err(BindError::UnknownAction("sav".into()))
        );
        assert_eq!(
            map.bind("nrmal", "x", "save"),
            Err(BindError::UnknownMode("nrmal".into()))
        );
        assert_eq!(
            map.bind("normal", "", "save"),
            Err(BindError::EmptySequence)
        );
        assert!(map.is_empty(), "nothing was bound");
    }

    #[test]
    fn ctrl_and_named_keys_bind() {
        let mut map = Keymap::new();
        map.bind("insert", "<c-s>", "save").unwrap();
        map.bind("normal", "<cr>", "completion").unwrap();

        assert_eq!(
            map.lookup(Mode::Insert, &[Key::ctrl('s')]),
            Match::Exact(Action::Save)
        );
        assert_eq!(
            map.lookup(Mode::Normal, &keys("<cr>")),
            Match::Exact(Action::RequestCompletion)
        );
    }

    #[test]
    fn every_documented_action_name_resolves() {
        // The names in `init.lua`'s comments and in the README have to actually exist. Checking a
        // representative one per family, because a name that only appears in documentation is a
        // promise the editor does not keep.
        for name in [
            "move_word_forward",
            "insert_mode",
            "open_below",
            "paste_before",
            "undo",
            "select_all",
            "toggle_soft_wrap",
            "save",
            "search_next",
            "goto_definition",
            "previous_diagnostic",
            "completion",
        ] {
            assert!(
                Action::from_name(name).is_some(),
                "`{name}` is documented but does not resolve"
            );
        }
    }
}

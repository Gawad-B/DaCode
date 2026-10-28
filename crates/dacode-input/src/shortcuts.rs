//! The keys the user has given commands, over the ones they are built with.
//!
//! [`COMMANDS`](crate::commands::COMMANDS) says what each command's keys *are*; this says what the
//! user changed. The Keys page edits it, `dacode.keys { … }` in `init.lua` is how it is written
//! down, and every surface that prints a shortcut — the palette, the help panel, the menus, the
//! which-key popup, the tooltips and the Keys page itself — reads a command's keys through
//! [`Shortcuts::keys`], so a key moved on the page stops being printed where it was.
//!
//! **Only shortcuts move.** A key held with ctrl or alt, or a function key ([`Key::is_shortcut`]),
//! means the same thing in every mode of either grammar, so it can be handed from one command to
//! another without touching anything else. Vim's own keys are a language, not a table: `u` is
//! undo because of what `d`, `c` and `.` are, and taking it away from undo is not moving a key. So
//! an override *replaces* a command's built-in shortcuts and leaves its Vim keys where they are —
//! undo given `ctrl-alt-z` is `ctrl-alt-z` and still `u`. Per-mode bindings of any key are
//! `dacode.map`, a separate and older layer ([`crate::keymap`]).
//!
//! **A sequence starts with `ctrl-k`.** It is already the prefix of `ctrl-k o` and the window
//! commands in both grammars, so nothing is lost by waiting for a second key after it. Any other
//! first key, if it could begin a sequence, would have to be held back to see what followed —
//! and its own meaning would arrive late or not at all.
//!
//! **The live set is per thread** ([`install`], [`current`]). The editor reads it on its one UI
//! thread, from the grammar and from every surface that prints a key; threading a reference to it
//! through each of those — the menus alone are built from a dozen functions — would have been a
//! hundred signatures that could each be given the wrong one. A thread rather than the process, so
//! tests, which cargo runs one to a thread, cannot see each other's keys.

use crate::action::Action;
use crate::commands::{self, Command};
use crate::key::Key;
use crate::keymap::Match;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

/// The user's keys for commands, replacing their built-in shortcuts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shortcuts {
    /// A command's name, and the keys it now has — each a sequence, in the order the user gave
    /// them. Empty means the command has no shortcut at all.
    overrides: BTreeMap<&'static str, Vec<Vec<Key>>>,
    /// Which [`install`] this is, so a cache built from one set can tell it is looking at another.
    generation: u64,
}

/// Why a key could not be given to a command. Said to the user in their terms, as config
/// problems are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutError {
    UnknownCommand(String),
    /// The keys did not parse to anything.
    Empty(String),
    /// The keys start with something that is not a shortcut — see the module documentation.
    NotAShortcut(String),
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShortcutError::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            ShortcutError::Empty(keys) => write!(f, "`{keys}` is not a key"),
            ShortcutError::NotAShortcut(keys) => write!(
                f,
                "`{keys}` does not start with ctrl, alt or a function key — dacode.map binds other \
                 keys, per mode"
            ),
        }
    }
}

/// The alternatives a key column lists, two spaces apart as the table writes them.
fn alternatives(column: &str) -> impl Iterator<Item = &str> {
    column
        .split("  ")
        .map(str::trim)
        .filter(|alt| !alt.is_empty())
}

/// A sequence as the table and `init.lua` write it.
pub fn notation(keys: &[Key]) -> String {
    keys.iter().map(|key| key.to_display()).collect()
}

impl Shortcuts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Every command the user has given keys to, by name, with those keys in `init.lua`'s notation.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Vec<String>)> + '_ {
        self.overrides
            .iter()
            .map(|(name, keys)| (*name, keys.iter().map(|keys| notation(keys)).collect()))
    }

    /// The user's keys for a command, or `None` when it has its built-in ones.
    pub fn get(&self, name: &str) -> Option<&[Vec<Key>]> {
        self.overrides.get(name).map(Vec::as_slice)
    }

    /// Gives a command exactly these keys, in `init.lua`'s notation — what `dacode.keys` does. An
    /// empty list leaves it with no shortcut.
    pub fn set(&mut self, name: &str, keys: &[&str]) -> Result<(), ShortcutError> {
        let command = commands::by_name(name)
            .ok_or_else(|| ShortcutError::UnknownCommand(name.to_string()))?;
        let mut parsed = Vec::new();
        for written in keys {
            let sequence = Key::parse_sequence(written);
            match sequence.first() {
                None => return Err(ShortcutError::Empty((*written).to_string())),
                Some(first) if !first.is_shortcut() => {
                    return Err(ShortcutError::NotAShortcut((*written).to_string()));
                }
                Some(_) => parsed.push(sequence),
            }
        }
        self.overrides.insert(command.name, parsed);
        Ok(())
    }

    /// Takes a command's override away: it has its built-in keys again, less any another command
    /// has been given.
    pub fn forget(&mut self, name: &str) {
        self.overrides.remove(name);
    }

    pub fn clear(&mut self) {
        self.overrides.clear();
    }

    /// What capturing a key on the Keys page does: the command's shortcuts become `keys` alone, and
    /// any other command the user had given `keys` loses it. A command that had `keys` *built in*
    /// needs nothing written: the user's layer is asked first, so it simply stops reaching it.
    pub fn assign(&mut self, command: &'static Command, keys: Vec<Key>) {
        for (name, list) in self.overrides.iter_mut() {
            if *name != command.name {
                list.retain(|existing| *existing != keys);
            }
        }
        self.overrides.insert(command.name, vec![keys]);
    }

    /// Leaves a command with no shortcut: Backspace on the Keys page.
    pub fn unbind(&mut self, command: &'static Command) {
        self.overrides.insert(command.name, Vec::new());
    }

    /// Puts a command's keys back as they were built: its override goes, and so does every key of
    /// its own that another command had been given — in either grammar, since an override applies
    /// to both. The ⟲ on a row.
    pub fn restore(&mut self, command: &'static Command) {
        self.overrides.remove(command.name);
        let own: Vec<Vec<Key>> = [command.keys, command.flat]
            .into_iter()
            .chain(terminal_keys(command.name))
            .flat_map(alternatives)
            .map(Key::parse_sequence)
            .filter(|keys| keys.first().is_some_and(|key| key.is_shortcut()))
            .collect();
        for list in self.overrides.values_mut() {
            list.retain(|keys| !own.contains(keys));
        }
    }

    /// Looks a sequence up in the user's layer, for the grammar.
    pub fn lookup(&self, keys: &[Key]) -> Match {
        if keys.is_empty() {
            return Match::None;
        }
        let mut prefix = false;
        for (name, list) in &self.overrides {
            for sequence in list {
                if !sequence.starts_with(keys) {
                    continue;
                }
                if sequence.len() == keys.len() {
                    return match Action::from_name(name) {
                        Some(action) => Match::Exact(action),
                        None => Match::None,
                    };
                }
                prefix = true;
            }
        }
        if prefix { Match::Prefix } else { Match::None }
    }

    /// Whether a built-in shortcut that resolved to `action` must be dropped, because the user gave
    /// that command other keys. The grammar asks this only of commands begun with a shortcut, so
    /// Vim's `u` still undoes when undo has been given `ctrl-alt-z`.
    pub fn suppresses(&self, action: &Action) -> bool {
        self.overrides
            .keys()
            .any(|name| Action::from_name(name).as_ref() == Some(action))
    }

    /// Whether one of the user's sequences takes `keys` from its built-in owner: the same keys, or
    /// a shorter sequence they begin with, which the grammar would act on first.
    fn claimed(&self, keys: &[Key]) -> bool {
        self.overrides
            .values()
            .flatten()
            .any(|user| keys.starts_with(user))
    }

    /// The keys that run a command now, in the table's notation, alternatives two spaces apart —
    /// the one lookup everything that prints a key goes through.
    ///
    /// The user's keys first, being the ones they chose; then the built-in alternatives still
    /// standing — every one, with no override, less those another command was given; with one,
    /// only the Vim keys, which an override does not replace.
    pub fn keys(&self, command: &Command, modal: bool) -> String {
        let mut shown: Vec<String> = Vec::new();
        let own = self.overrides.get(command.name);
        if let Some(list) = own {
            shown.extend(list.iter().map(|keys| notation(keys)));
        }
        for alternative in default_alternatives(command, modal) {
            let keys = Key::parse_sequence(alternative);
            let shortcut = keys.first().is_some_and(|key| key.is_shortcut());
            let standing = if shortcut {
                own.is_none() && !self.claimed(&keys)
            } else {
                true
            };
            if standing && !shown.iter().any(|already| already == alternative) {
                shown.push(alternative.to_string());
            }
        }
        shown.join("  ")
    }

    /// Whether a command's keys are not the ones it was built with — what puts a ⟲ on its row.
    pub fn differs(&self, command: &Command, modal: bool) -> bool {
        self.keys(command, modal) != default_keys(command, modal)
    }

    /// Which command `keys` runs now, in this grammar: the user's, or the built-in owner it has
    /// not been taken from. What the Keys page warns about before giving a key away.
    pub fn owner(&self, keys: &[Key], modal: bool) -> Option<&'static Command> {
        if let Some((name, _)) = self
            .overrides
            .iter()
            .find(|(_, list)| list.iter().any(|user| user.as_slice() == keys))
        {
            return commands::by_name(name);
        }
        if self.claimed(keys) {
            return None;
        }
        commands::available(modal).find(|command| {
            !self.overrides.contains_key(command.name)
                && default_alternatives(command, modal)
                    .any(|alternative| Key::parse_sequence(alternative) == keys)
        })
    }
}

/// The keys only a focused terminal answers, which `dacode::terminal::route` keeps from the shell.
///
/// Not in the table's own columns, which promise that the *grammar* resolves them and are tested
/// for it; the binary has a test that `route` answers each of these with its command.
pub const TERMINAL_KEYS: &[(&str, &str)] = &[
    ("next_terminal", "<c-pgdn>"),
    ("previous_terminal", "<c-pgup>"),
    ("terminal_copy", "<c-s-c>"),
    ("terminal_paste", "<c-s-v>"),
];

fn terminal_keys(name: &str) -> Option<&'static str> {
    TERMINAL_KEYS
        .iter()
        .find(|(command, _)| *command == name)
        .map(|(_, keys)| *keys)
}

fn default_alternatives(command: &Command, modal: bool) -> impl Iterator<Item = &'static str> {
    alternatives(command.default_keys(modal)).chain(terminal_keys(command.name))
}

/// A command's keys as it was built, in the same form [`Shortcuts::keys`] gives.
pub fn default_keys(command: &Command, modal: bool) -> String {
    default_alternatives(command, modal)
        .collect::<Vec<_>>()
        .join("  ")
}

/// What can follow `prefix` now, for the which-key popup: the built-in continuations whose keys
/// still do what they say, and every sequence the user gave a command under the prefix.
///
/// Only a shortcut prefix — `ctrl-k` — can lose or gain a continuation; `g`, `z` and the rest are
/// Vim's grammar, which an override does not touch, and are passed through as they are.
pub fn continuations(prefix: &str, modal: bool) -> Vec<(String, String)> {
    let built_in = commands::continuations(prefix, modal);
    let head = Key::parse_sequence(prefix);
    if !head.first().is_some_and(|key| key.is_shortcut()) {
        return built_in
            .iter()
            .map(|(key, what)| ((*key).to_string(), (*what).to_string()))
            .collect();
    }
    let shortcuts = current();
    let mode = if modal {
        dacode_theme::Mode::Normal
    } else {
        dacode_theme::Mode::Insert
    };
    let mut shown: Vec<(String, String)> = built_in
        .iter()
        .filter(|(key, _)| {
            let mut sequence = head.clone();
            sequence.extend(Key::parse_sequence(key));
            if shortcuts.claimed(&sequence) {
                return false;
            }
            // Asked of the real grammar, which drops a built-in whose command was moved.
            let mut grammar = crate::grammar::Grammar::new();
            grammar.set_modal(modal);
            sequence.into_iter().fold(false, |_, key| {
                matches!(
                    grammar.feed(mode, key),
                    crate::grammar::Resolution::Action(_)
                )
            })
        })
        .map(|(key, what)| ((*key).to_string(), (*what).to_string()))
        .collect();
    for (name, list) in &shortcuts.overrides {
        for sequence in list {
            if sequence.len() > head.len() && sequence.starts_with(&head) {
                let what = commands::by_name(name).map_or(*name, |command| command.what);
                shown.push((notation(&sequence[head.len()..]), what.to_string()));
            }
        }
    }
    shown
}

thread_local! {
    static LIVE: RefCell<Rc<Shortcuts>> = RefCell::new(Rc::new(Shortcuts::default()));
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// Makes `shortcuts` the set this thread's editor runs and shows.
pub fn install(mut shortcuts: Shortcuts) {
    let generation = GENERATION.with(|counter| {
        counter.set(counter.get() + 1);
        counter.get()
    });
    shortcuts.generation = generation;
    LIVE.with(|live| *live.borrow_mut() = Rc::new(shortcuts));
}

/// The set this thread's editor runs and shows.
pub fn current() -> Rc<Shortcuts> {
    LIVE.with(|live| live.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> &'static Command {
        commands::by_name(name).unwrap()
    }

    fn keys(text: &str) -> Vec<Key> {
        Key::parse_sequence(text)
    }

    #[test]
    fn a_new_key_replaces_the_built_in_shortcut_and_leaves_vims_keys_alone() {
        // Replacing is the point: a key moved on the Keys page that still left the old one working
        // and printed would be an addition the page called a change. Vim's `u` stays because it is
        // grammar, not a shortcut.
        let mut shortcuts = Shortcuts::new();
        shortcuts.assign(command("undo"), keys("<c-a-z>"));

        assert_eq!(shortcuts.keys(command("undo"), false), "<c-a-z>");
        assert_eq!(shortcuts.keys(command("undo"), true), "<c-a-z>  u");
        assert!(shortcuts.suppresses(&Action::Undo));
        assert_eq!(
            shortcuts.lookup(&keys("<c-a-z>")),
            Match::Exact(Action::Undo)
        );
    }

    #[test]
    fn a_key_given_away_stops_being_printed_beside_its_old_command() {
        // `ctrl-w` closes the file until it is given to something else; after that, a palette that
        // still said "close this file  Ctrl+W" would be teaching a key that now does another thing.
        let mut shortcuts = Shortcuts::new();
        shortcuts.assign(command("toggle_comment"), keys("<c-w>"));

        assert_eq!(shortcuts.keys(command("close_buffer"), false), "<c-f4>");
        assert_eq!(
            shortcuts.owner(&keys("<c-w>"), false).map(|c| c.name),
            Some("toggle_comment")
        );
        assert!(shortcuts.differs(command("close_buffer"), false));
    }

    #[test]
    fn taking_a_key_from_another_override_takes_it_from_that_list_alone() {
        let mut shortcuts = Shortcuts::new();
        shortcuts
            .set("save", &["<c-a-s>", "<f5>"])
            .expect("both are shortcuts");
        shortcuts.assign(command("quit"), keys("<f5>"));

        assert_eq!(shortcuts.keys(command("save"), false), "<c-a-s>");
        assert_eq!(shortcuts.keys(command("quit"), false), "<f5>");
    }

    #[test]
    fn restoring_a_command_gives_it_back_the_keys_others_took() {
        // The ⟲ promises "as it was built". Leaving `ctrl-w` with toggle_comment would restore
        // close_buffer to a key that does something else.
        let mut shortcuts = Shortcuts::new();
        shortcuts.assign(command("toggle_comment"), keys("<c-w>"));
        shortcuts.restore(command("close_buffer"));

        assert_eq!(
            shortcuts.keys(command("close_buffer"), false),
            "<c-w>  <c-f4>"
        );
        assert!(!shortcuts.differs(command("close_buffer"), false));
        assert_eq!(shortcuts.keys(command("toggle_comment"), false), "");
    }

    #[test]
    fn unbinding_leaves_a_command_with_no_shortcut() {
        let mut shortcuts = Shortcuts::new();
        shortcuts.unbind(command("close_buffer"));
        assert_eq!(shortcuts.keys(command("close_buffer"), false), "");
        assert!(shortcuts.suppresses(&Action::CloseBuffer));
    }

    #[test]
    fn a_ctrl_k_sequence_is_a_prefix_until_its_second_key() {
        let mut shortcuts = Shortcuts::new();
        shortcuts.assign(command("toggle_comment"), keys("<c-k><c-c>"));
        assert_eq!(shortcuts.lookup(&keys("<c-k>")), Match::Prefix);
        assert_eq!(
            shortcuts.lookup(&keys("<c-k><c-c>")),
            Match::Exact(Action::ToggleComment)
        );
        assert_eq!(shortcuts.lookup(&keys("<c-k>o")), Match::None);
    }

    #[test]
    fn config_cannot_hand_a_command_a_letter() {
        // A letter given to a command would have to be taken from typing in every mode; that is
        // what `dacode.map` and its modes are for.
        let mut shortcuts = Shortcuts::new();
        assert_eq!(
            shortcuts.set("save", &["w"]),
            Err(ShortcutError::NotAShortcut("w".into()))
        );
        assert_eq!(
            shortcuts.set("sav", &["<c-s>"]),
            Err(ShortcutError::UnknownCommand("sav".into()))
        );
        assert!(shortcuts.is_empty());
    }

    #[test]
    fn the_terminal_keys_are_shown_for_their_commands() {
        let shortcuts = Shortcuts::new();
        assert_eq!(shortcuts.keys(command("next_terminal"), false), "<c-pgdn>");
        assert_eq!(
            commands::pretty(&shortcuts.keys(command("terminal_copy"), false)),
            "Ctrl+Shift+C"
        );
    }

    #[test]
    fn the_which_key_popup_offers_what_ctrl_k_leads_to_now() {
        // Moving "open a folder" off `ctrl-k o` and giving toggle_comment `ctrl-k ctrl-c`: the popup
        // that teaches the prefix must stop offering the first and start offering the second.
        let mut shortcuts = Shortcuts::new();
        shortcuts.assign(command("open_folder"), keys("<f6>"));
        shortcuts.assign(command("toggle_comment"), keys("<c-k><c-c>"));
        install(shortcuts);
        let offered = continuations("<c-k>", false);
        install(Shortcuts::new());
        assert!(!offered.iter().any(|(key, _)| key == "o"), "{offered:?}");
        assert!(offered.iter().any(|(key, _)| key == "v"), "{offered:?}");
        assert!(
            offered
                .iter()
                .any(|(key, what)| key == "<c-c>" && what == command("toggle_comment").what),
            "{offered:?}"
        );
    }

    #[test]
    fn installing_a_set_is_seen_by_this_thread_and_bumps_its_generation() {
        let before = current().generation();
        let mut shortcuts = Shortcuts::new();
        shortcuts.unbind(command("save"));
        install(shortcuts);
        assert!(current().generation() > before);
        assert_eq!(command("save").shown(false), "");
        install(Shortcuts::new());
        assert_eq!(command("save").shown(false), "Ctrl+S");
    }
}

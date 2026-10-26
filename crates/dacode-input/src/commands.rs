//! Every command the editor has, in one table, with the keys that run it.
//!
//! This is the data behind the three things that make an editor learnable: the command palette lists
//! it, the help panel groups it, and the which-key popup filters it by what you have typed so far.
//! One table rather than three, because three would disagree — and a palette that lies about which
//! key runs something is worse than one that says nothing.
//!
//! The rule the test at the bottom enforces: **every name [`Action::from_name`] accepts appears
//! here**. Adding a command without a description is how a command palette fills up with entries
//! nobody can interpret, and it is the failure mode of every editor that generates its palette from
//! an enum.
//!
//! `keys` is what to press with the modal grammar on; `flat` is what to press with it off. Empty
//! means the command has no key in that mode and the palette is the only way to it — which is the
//! honest thing for the palette to show, and better than inventing a binding.

use crate::action::Action;

/// One command: what it is called, what it does, and how to reach it.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Command {
    /// The name config binds and the palette searches. Public API — see [`Action::from_name`].
    pub name: &'static str,
    /// What it does, in the user's words rather than the code's. Lower case, no full stop: it is a
    /// list entry, not a sentence.
    pub what: &'static str,
    /// Which section of the help panel it belongs in.
    pub group: Group,
    /// The keys with Vim's grammar on.
    pub keys: &'static str,
    /// The keys with it off.
    pub flat: &'static str,
    /// Whether the command only means anything with the modal grammar on.
    ///
    /// Deliberately not the same as "has no key when it is off". `previous_pane` has no key in
    /// either grammar and the palette is the only way to reach it, which is correct. `visual_mode`
    /// is a different thing: with the grammar off there *is* no visual mode, and listing "start
    /// selecting" for someone who turned Vim off teaches them a Vim idea in the one surface that
    /// was supposed to be free of them.
    pub modal_only: bool,
}

impl Command {
    /// The keys to show for the mode the editor is in.
    pub fn keys_for(&self, modal: bool) -> &'static str {
        if modal { self.keys } else { self.flat }
    }

    /// The keys as a person reads them: Vim's notation for someone using Vim's grammar, and
    /// `Ctrl+Shift+P` for everyone else (see [`pretty`]).
    pub fn shown(&self, modal: bool) -> String {
        if modal {
            self.keys.to_string()
        } else {
            pretty(self.flat)
        }
    }

    /// The action this command runs.
    pub fn action(&self) -> Option<Action> {
        Action::from_name(self.name)
    }
}

/// Keys written the way a desktop application writes them: `<c-s-p>` becomes `Ctrl+Shift+P`.
///
/// The table uses Vim's notation because the modal grammar is built on it, and someone who chose
/// that grammar reads it fluently. Someone who did not has never seen `<c-s-p>`, and a tooltip that
/// teaches a key in a notation they cannot read teaches nothing. A chord followed by another key —
/// `<c-k>o` — keeps the gap between them, since that is two presses.
pub fn pretty(keys: &str) -> String {
    let mut out = String::new();
    let mut rest = keys;
    while let Some(first) = rest.chars().next() {
        let group = (first == '<')
            .then(|| rest[1..].find('>').map(|end| &rest[1..1 + end]))
            .flatten()
            .filter(|inner| !inner.is_empty() && !inner.contains('<'));
        let piece = match group {
            Some(inner) => {
                rest = &rest[inner.len() + 2..];
                chord(inner)
            }
            None => {
                rest = &rest[first.len_utf8()..];
                match first {
                    ' ' => continue,
                    other => other.to_uppercase().to_string(),
                }
            }
        };
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(&piece);
    }
    out
}

fn chord(inner: &str) -> String {
    let parts: Vec<&str> = inner.split('-').collect();
    // `<c-->` would split into an empty last part; the key there is the dash itself.
    let (modifiers, key) = match parts.split_last() {
        Some((&"", before)) if !before.is_empty() => (&before[..before.len() - 1], "-"),
        Some((key, before)) => (before, *key),
        None => (&[][..], inner),
    };
    let mut names: Vec<String> = modifiers
        .iter()
        .map(|modifier| {
            match *modifier {
                "c" => "Ctrl",
                "s" => "Shift",
                "a" => "Alt",
                "m" | "d" => "Super",
                other => other,
            }
            .to_string()
        })
        .collect();
    let key = match key {
        "bs" => "Backspace".to_string(),
        "del" => "Delete".to_string(),
        "cr" | "enter" => "Enter".to_string(),
        "esc" => "Esc".to_string(),
        "tab" => "Tab".to_string(),
        "space" => "Space".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        other if other.starts_with('f') && other[1..].parse::<u8>().is_ok() => other.to_uppercase(),
        other => other.to_uppercase(),
    };
    names.push(key);
    names.join("+")
}

#[cfg(test)]
mod pretty_tests {
    use super::pretty;

    #[test]
    fn chords_read_the_way_a_desktop_application_writes_them() {
        assert_eq!(pretty("<c-s-p>"), "Ctrl+Shift+P");
        assert_eq!(pretty("<f8>"), "F8");
        assert_eq!(pretty("<s-f8>"), "Shift+F8");
        assert_eq!(pretty("<c-bs>"), "Ctrl+Backspace");
        assert_eq!(pretty("<a-↑>"), "Alt+↑");
        assert_eq!(pretty("<c-,>"), "Ctrl+,");
    }

    #[test]
    fn a_sequence_keeps_its_presses_apart() {
        assert_eq!(pretty("<c-k>o"), "Ctrl+K O");
        assert_eq!(pretty("gD"), "G D");
    }

    #[test]
    fn a_bracket_that_opens_no_chord_is_just_a_bracket() {
        // Vim's outdent is two less-than signs, not a chord with nothing in it.
        assert_eq!(pretty("<<"), "< <");
        assert_eq!(pretty(""), "");
    }
}

/// The sections of the help panel, in the order they are shown.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Files,
    Editing,
    Selection,
    Navigation,
    Search,
    Language,
    View,
    Panes,
}

impl Group {
    pub const ALL: [Group; 8] = [
        Group::Files,
        Group::Editing,
        Group::Selection,
        Group::Navigation,
        Group::Search,
        Group::Language,
        Group::View,
        Group::Panes,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Files => "files",
            Group::Editing => "editing",
            Group::Selection => "selection and cursors",
            Group::Navigation => "moving around",
            Group::Search => "finding things",
            Group::Language => "the language server",
            Group::View => "the view",
            Group::Panes => "panes and panels",
        }
    }
}

/// Shorthand, so the table below reads as a table.
const fn command(
    name: &'static str,
    what: &'static str,
    group: Group,
    keys: &'static str,
    flat: &'static str,
) -> Command {
    Command {
        name,
        what,
        group,
        keys,
        flat,
        modal_only: false,
    }
}

/// A command that exists only while the modal grammar is on.
///
/// It takes no `flat` because there could not be one: these are the operations that are *about* the
/// grammar — a mode to enter, a change to repeat, a macro to replay, a paste that has a side to
/// land on.
const fn modal(
    name: &'static str,
    what: &'static str,
    group: Group,
    keys: &'static str,
) -> Command {
    Command {
        name,
        what,
        group,
        keys,
        flat: "",
        modal_only: true,
    }
}

use Group::*;

/// Every command, in palette order.
///
/// Ordered by group and then by how often it is wanted, because an empty query shows this list as it
/// stands and the first screen of it is what someone browsing actually reads.
pub const COMMANDS: &[Command] = &[
    // ---- files -------------------------------------------------------------------------
    command("save", "save this file", Files, ":w  <c-s>", "<c-s>"),
    command(
        "save_as",
        "save this file somewhere else",
        Files,
        "",
        "<c-s-s>",
    ),
    command(
        "toggle_autosave",
        "save automatically, or do not",
        Files,
        "",
        "",
    ),
    command(
        "open_folder",
        "open a folder as the project",
        Files,
        "<c-k>o",
        "<c-k>o",
    ),
    command("close_buffer", "close this file", Files, "<c-f4>", "<c-f4>"),
    // No keys: these act on what the tree has selected, and the tree is a mouse surface. The
    // palette and the tree's own right-click menu are the honest routes, and inventing a chord for
    // "delete a file" would be actively unkind.
    command("new_file", "new file here", Files, "", ""),
    command("rename_file", "rename this file", Files, "", ""),
    command("delete_file", "delete this file", Files, "", ""),
    // Where someone looks when the statusline says there is no server: the message names the command
    // and this runs it. Nothing is installed without being asked twice.
    command(
        "install_language_server",
        "install a language server for this file",
        Files,
        "",
        "",
    ),
    command("quit", "quit", Files, ":q  <c-q>", "<c-q>"),
    // ---- editing -----------------------------------------------------------------------
    command("copy", "copy", Editing, "y", "<c-c>"),
    command("cut", "cut", Editing, "d", "<c-x>"),
    command("undo", "undo", Editing, "u", "<c-z>"),
    command("redo", "redo", Editing, "<c-r>", "<c-y>  <c-s-z>"),
    command("paste", "paste after the cursor", Editing, "p", "<c-v>"),
    modal("paste_before", "paste before the cursor", Editing, "P"),
    command(
        "toggle_comment",
        "comment or uncomment these lines",
        Editing,
        "",
        "<c-/>",
    ),
    command("move_line_up", "move these lines up", Editing, "", "<a-↑>"),
    command(
        "move_line_down",
        "move these lines down",
        Editing,
        "",
        "<a-↓>",
    ),
    command(
        "duplicate_line_up",
        "copy these lines above",
        Editing,
        "",
        "<a-s-↑>",
    ),
    command(
        "duplicate_line_down",
        "copy these lines below",
        Editing,
        "",
        "<a-s-↓>",
    ),
    command("delete_line", "delete this line", Editing, "dd", "<c-s-k>"),
    command(
        "delete_word_backward",
        "delete the word before the cursor",
        Editing,
        "db",
        "<c-bs>",
    ),
    command(
        "delete_word_forward",
        "delete the word after the cursor",
        Editing,
        "dw",
        "<c-del>",
    ),
    command("indent", "indent", Editing, ">>", "<tab>"),
    command("outdent", "outdent", Editing, "<<", "<s-tab>"),
    command("format", "reformat the file", Editing, "", ""),
    command(
        "expand_snippet",
        "expand the snippet before the cursor",
        Editing,
        "<tab>",
        "<tab>",
    ),
    modal("repeat_change", "repeat the last change", Editing, "."),
    modal("replay_macro", "replay a recorded macro", Editing, "@"),
    // ---- selection ---------------------------------------------------------------------
    command(
        "select_all",
        "select the whole file",
        Selection,
        "ggVG",
        "<c-a>",
    ),
    command("select_line", "select this line", Selection, "V", "<c-l>"),
    command(
        "select_next_occurrence",
        "select the word, then the next copy of it",
        Selection,
        "",
        "<c-d>",
    ),
    command(
        "select_all_occurrences",
        "select every copy of it",
        Selection,
        "",
        "<c-s-l>",
    ),
    command(
        "add_caret_below",
        "add a cursor below",
        Selection,
        "<c-a-↓>",
        "<c-a-↓>",
    ),
    command(
        "add_caret_above",
        "add a cursor above",
        Selection,
        "<c-a-↑>",
        "<c-a-↑>",
    ),
    command(
        "split_selection_into_lines",
        "one cursor per selected line",
        Selection,
        "s",
        "",
    ),
    command(
        "keep_primary_selection",
        "drop the other cursors",
        Selection,
        ",",
        "",
    ),
    command(
        "collapse_selections",
        "collapse to one cursor",
        Selection,
        "<esc>",
        "<esc>",
    ),
    modal("visual_mode", "start selecting", Selection, "v"),
    // ---- navigation --------------------------------------------------------------------
    command(
        "move_file_start",
        "go to the top",
        Navigation,
        "gg",
        "<c-home>",
    ),
    command(
        "move_file_end",
        "go to the bottom",
        Navigation,
        "G",
        "<c-end>",
    ),
    command(
        "move_matching_bracket",
        "jump to the matching bracket",
        Navigation,
        "%",
        "",
    ),
    command(
        "jump_back",
        "back to where you were",
        Navigation,
        "<c-o>",
        "<a-←>",
    ),
    command(
        "jump_forward",
        "forward again",
        Navigation,
        "<c-i>",
        "<a-→>",
    ),
    command(
        "toggle_fold",
        "fold or unfold this block",
        Navigation,
        "za",
        "",
    ),
    command("fold_all", "fold everything", Navigation, "zM", ""),
    command("unfold_all", "unfold everything", Navigation, "zR", ""),
    // ---- search ------------------------------------------------------------------------
    command("search", "find in this file", Search, "/", "<c-f>"),
    command(
        "replace",
        "find and replace in this file",
        Search,
        "<c-h>",
        "<c-h>",
    ),
    command("search_next", "next match", Search, "n", "<f3>"),
    command("search_previous", "previous match", Search, "N", "<s-f3>"),
    command(
        "search_project",
        "find in every file",
        Search,
        "",
        "<c-s-f>",
    ),
    command("command", "type a command", Search, ":", "<c-g>"),
    command(
        "command_palette",
        "every command, searchable",
        Search,
        "<c-s-p>",
        "<c-s-p>",
    ),
    command("help", "which keys do what", Search, "<f1>", "<f1>"),
    // ---- language server ---------------------------------------------------------------
    command(
        "goto_definition",
        "go to the definition",
        Language,
        "gd",
        "<f12>",
    ),
    command(
        "find_references",
        "find every use of this",
        Language,
        "gu",
        "<s-f12>",
    ),
    command(
        "goto_symbol",
        "go to a symbol in this file",
        Language,
        "gs",
        "<c-s-o>",
    ),
    command(
        "goto_symbol_in_project",
        "go to a symbol anywhere in the project",
        Language,
        "gS",
        "<c-t>",
    ),
    command(
        "next_change",
        "next change since the last commit",
        Language,
        "]c",
        "<f7>",
    ),
    command(
        "previous_change",
        "previous change since the last commit",
        Language,
        "[c",
        "<s-f7>",
    ),
    command(
        "revert_hunk",
        "undo this change, back to how it was committed",
        Language,
        "]C",
        "",
    ),
    command("hover", "what is this?", Language, "K", ""),
    command("rename", "rename this everywhere", Language, "gr", "<f2>"),
    command(
        "code_action",
        "what can be done about this?",
        Language,
        "ga",
        "",
    ),
    command(
        "completion",
        "suggest a completion",
        Language,
        "<c-space>",
        "<c-space>",
    ),
    command("next_diagnostic", "next problem", Language, "]d", "<f8>"),
    command(
        "previous_diagnostic",
        "previous problem",
        Language,
        "[d",
        "<s-f8>",
    ),
    command(
        "toggle_diagnostics",
        "list every problem",
        Language,
        "gD",
        "",
    ),
    command("source_control", "show source control", View, "", ""),
    command("run_and_debug", "show run and debug", View, "", ""),
    command("notifications", "show recent messages", View, "", ""),
    command("share", "copy link to line", Files, "", ""),
    command("run", "copy the run command", Files, "", ""),
    command(
        "toggle_inlay_hints",
        "show or hide inlay hints",
        Language,
        "",
        "",
    ),
    // ---- the view ----------------------------------------------------------------------
    command("settings", "settings", View, "<c-,>", "<c-,>"),
    command(
        "toggle_tree",
        "show or hide the file tree",
        View,
        "<c-e>",
        "<c-e>",
    ),
    command(
        "focus_explorer",
        "move the keys to the file tree",
        View,
        "<c-s-e>",
        "<c-s-e>",
    ),
    command(
        "focus_editor",
        "move the keys back to the text",
        View,
        "<c-1>",
        "<c-1>",
    ),
    command(
        "toggle_theme",
        "switch between light and dark",
        View,
        "",
        "",
    ),
    command(
        "toggle_soft_wrap",
        "wrap long lines, or do not",
        View,
        "",
        "",
    ),
    // No non-modal key by design: no conventional editor binds horizontal scroll to one, the wheel
    // and reveal-on-cursor cover it, and inventing a chord is worse than the palette saying
    // honestly that there is none.
    command("scroll_left", "look left along a long line", View, "zh", ""),
    command(
        "scroll_right",
        "look right along a long line",
        View,
        "zl",
        "",
    ),
    command("increase_font_size", "bigger text", View, "", ""),
    command("decrease_font_size", "smaller text", View, "", ""),
    command("menu", "the menu bar", View, "<f10>", "<f10>"),
    command(
        "context_menu",
        "what can I do here?",
        View,
        "<s-f10>",
        "<s-f10>",
    ),
    // ---- panes -------------------------------------------------------------------------
    command("split_vertical", "split beside", Panes, "<c-w>v", "<c-w>v"),
    command("split_horizontal", "split below", Panes, "<c-w>s", "<c-w>s"),
    command("close_pane", "close this pane", Panes, "<c-w>c", "<c-w>c"),
    command(
        "next_pane",
        "focus the next pane",
        Panes,
        "<c-w>w",
        "<c-w>w",
    ),
    command("previous_pane", "focus the previous pane", Panes, "", ""),
];

/// The commands in a group, for the help panel.
pub fn in_group(group: Group) -> impl Iterator<Item = &'static Command> {
    COMMANDS
        .iter()
        .filter(move |command| command.group == group)
}

/// Every command that means something in this grammar.
///
/// The palette listed the whole table regardless, so someone who had turned Vim off could still
/// search it and find "start selecting", "repeat the last change" and "replay a recorded macro" —
/// three ideas that do not exist in the editor they chose, in the one surface whose whole job is to
/// tell them what the editor can do.
pub fn available(modal: bool) -> impl Iterator<Item = &'static Command> {
    COMMANDS
        .iter()
        .filter(move |command| modal || !command.modal_only)
}

/// Looks a command up by name.
pub fn by_name(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.name == name)
}

/// What can follow a prefix, for the which-key popup.
///
/// Typed out rather than derived from the grammar because the grammar is a state machine over
/// characters and cannot say what it *would* accept next — and a popup that guessed would teach
/// people keys that do not work. The test below checks every entry actually resolves.
///
/// Takes the grammar because two of these prefixes are Vim's alone: `"` chooses a register and `q`
/// records a macro, and `feed_flat` can produce neither. A popup that offered them with the grammar
/// off would be teaching keys that cannot be pressed.
pub fn continuations(prefix: &str, modal: bool) -> &'static [(&'static str, &'static str)] {
    if !modal && matches!(prefix, "\"" | "q" | "g" | "z" | "]" | "[") {
        return &[];
    }
    match prefix {
        "<c-w>" => &[
            ("v", "split beside"),
            ("s", "split below"),
            ("c", "close this pane"),
            ("w", "focus the next pane"),
        ],
        "<c-k>" => &[("<c-o>", "open a folder")],
        "g" => &[
            ("g", "go to the top"),
            ("d", "go to the definition"),
            ("u", "find every use of this"),
            ("r", "rename this everywhere"),
            ("a", "what can be done about this?"),
            ("D", "list every problem"),
            ("h", "line start"),
            ("l", "line end"),
            ("e", "end of file"),
        ],
        "z" => &[
            ("a", "fold or unfold this block"),
            ("R", "unfold everything"),
            ("M", "fold everything"),
            ("w", "wrap long lines, or do not"),
            ("h", "look left along a long line"),
            ("l", "look right along a long line"),
        ],
        "]" => &[("d", "next problem")],
        "[" => &[("d", "previous problem")],
        "\"" => &[("a…z", "use that register for the next yank or paste")],
        "q" => &[("a…z", "record a macro into that register")],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_names_an_action_that_exists() {
        // The palette runs these by name. One that does not resolve is a row that does nothing when
        // chosen, which is the worst kind of entry in a list whose whole job is to be trustworthy.
        for command in COMMANDS {
            assert!(
                command.action().is_some(),
                "`{}` is in the palette but `Action::from_name` does not know it",
                command.name
            );
        }
    }

    /// Names that are deliberately not commands.
    ///
    /// Motions, mode switches and the single-character edits are the *grammar*, not things anybody
    /// would choose from a list — "move left" in a command palette is a joke about command palettes.
    /// They stay bindable by name in config, which is what they are for. Everything else must
    /// appear, and the test below is what makes forgetting impossible.
    const NOT_COMMANDS: &[&str] = &[
        "move_left",
        "move_right",
        "move_up",
        "move_down",
        "move_word_forward",
        "move_word_backward",
        "move_word_end",
        "move_line_start",
        "move_line_end",
        "move_first_non_blank",
        "move_paragraph_forward",
        "move_paragraph_backward",
        "normal_mode",
        "insert_mode",
        "replace_mode",
        "insert_before",
        "insert_after",
        "insert_line_start",
        "insert_line_end",
        "open_below",
        "open_above",
        "newline",
        "delete_backward",
        "delete_forward",
        "dismiss_info",
        "search_backward",
    ];

    #[test]
    fn every_action_name_is_in_the_palette_or_deliberately_not() {
        // The direction that rots: adding an action and forgetting the table leaves a command nobody
        // can find, which is how every generated palette ends up incomplete. `NAMES` is the list
        // `from_name` accepts; the exclusions above are a decision, written down.
        for name in Action::NAMES {
            assert!(
                by_name(name).is_some() || NOT_COMMANDS.contains(name),
                "`{name}` is an action with no palette entry — add it to COMMANDS, or to \
                 NOT_COMMANDS with a reason"
            );
        }
    }

    #[test]
    fn nothing_is_excluded_that_does_not_exist() {
        // An exclusion list is a place where dead names accumulate. This one cannot.
        for name in NOT_COMMANDS {
            assert!(
                Action::NAMES.contains(name),
                "`{name}` is excluded from the palette but is not an action any more"
            );
            assert!(
                by_name(name).is_none(),
                "`{name}` is both excluded and listed"
            );
        }
    }

    #[test]
    fn every_advertised_non_modal_key_actually_does_something() {
        // The check that makes the palette and the help panel trustworthy: a key printed beside a
        // command has to resolve, in the grammar, to an action. An editor that tells a beginner to
        // press something that does nothing has done worse than tell them nothing — and this table
        // exists precisely because beginners were being told nothing.
        use crate::grammar::{Grammar, Resolution};
        use crate::key::Key;
        use dacode_theme::Mode;

        let mut wrong = Vec::new();
        for command in COMMANDS {
            for keys in command.flat.split_whitespace() {
                let mut grammar = Grammar::new();
                grammar.set_modal(false);
                let mut resolved = false;
                for key in Key::parse_sequence(keys) {
                    match grammar.feed(Mode::Insert, key) {
                        Resolution::Action(_) => resolved = true,
                        Resolution::Unhandled => break,
                        Resolution::Pending(_) => {}
                    }
                }
                if !resolved {
                    wrong.push(format!("{} says {keys}", command.name));
                }
            }
        }

        assert!(wrong.is_empty(), "keys that do nothing: {wrong:#?}");
    }

    #[test]
    fn no_two_commands_share_a_name() {
        let mut seen: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        seen.sort_unstable();
        let count = seen.len();
        seen.dedup();
        assert_eq!(count, seen.len(), "a name appears twice");
    }

    #[test]
    fn a_description_reads_as_a_list_entry() {
        // The palette is a list, so the descriptions have to scan as one: no trailing full stops,
        // nothing capitalised for no reason, nothing empty.
        for command in COMMANDS {
            assert!(
                !command.what.is_empty(),
                "{} has no description",
                command.name
            );
            assert!(
                !command.what.ends_with('.'),
                "{}: descriptions are list entries, not sentences",
                command.name
            );
        }
    }

    #[test]
    fn every_continuation_describes_something_real() {
        // A which-key popup exists to be trusted. An entry here that no command matches is the popup
        // teaching a key that does nothing.
        for prefix in ["<c-w>", "<c-k>", "g", "z", "]", "["] {
            assert!(
                !continuations(prefix, true).is_empty(),
                "{prefix} lost its continuations"
            );
            for (key, what) in continuations(prefix, true) {
                assert!(!key.is_empty() && !what.is_empty());
            }
        }
        assert!(
            continuations("d", true).is_empty(),
            "an operator is not a prefix with a menu: `d` waits for a motion, and listing every \
             motion would be the cheatsheet, not a hint"
        );
    }
}

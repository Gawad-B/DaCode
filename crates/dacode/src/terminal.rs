//! The terminal panel, as the editor sees it: whether it is open, how tall it is, its tabs, what
//! the shown one last showed, how its cells are coloured, and which keys it keeps.
//!
//! The shell itself is not here. It is a process, and the executor owns no processes — the event
//! loop holds the [`dacode_terminal::Terminal`]s beside the language servers, and hands this module a
//! [`Screen`] to draw. That split is also what lets the screenshot harness draw a terminal with no
//! shell behind it: it fills [`Panel::screen`] from an [`dacode_terminal::Emulator`] fed a fixed
//! transcript.

use dacode_input::{Action, Key, KeyCode};
use dacode_terminal::{Cell, Color, Screen};
use dacode_theme::{Mode, Oklch, Theme};

/// The share of the editor's height the panel opens at: VSCode's.
pub const DEFAULT_FRACTION: f32 = 0.4;

/// The panel's state on [`crate::chrome::EditorState`].
#[derive(Debug, Clone)]
pub struct Panel {
    /// Shown under the editor. Hiding it leaves the shells running, as VSCode does: closing a view
    /// is not a decision to kill whatever is running in it.
    pub open: bool,
    /// What the shown shell last showed, copied out once per frame that needed it. `None` before
    /// the first copy, which draws as an empty panel.
    pub screen: Option<Screen>,
    /// One title per shell, in tab order: what the shell set with `OSC 0`, or its program's name.
    pub tabs: Vec<String>,
    /// Which tab is shown and has the keys.
    pub active: usize,
    /// How much of the editor's height the panel asks for. Dragging its top edge sets it; the
    /// pixels are [`crate::layout::terminal_height`]'s, which clamps it so neither the panel nor
    /// the code above it can be squeezed to nothing.
    pub fraction: f32,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            screen: None,
            tabs: Vec::new(),
            active: 0,
            fraction: DEFAULT_FRACTION,
        }
    }
}

/// What a tab is called: the title the shell set, or else its program's name — `zsh`, `pwsh` —
/// which is what tells two prompts apart before either has said anything.
pub fn tab_title(title: Option<String>, shell: &dacode_terminal::ShellCommand) -> String {
    title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(&shell.program)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| shell.program.clone())
        })
}

/// The line height of a terminal row, in ems of the code size.
///
/// Tighter than the code's own generous 1.9: a terminal is read in blocks — a listing, a
/// backtrace, a table some program drew with box characters — and box-drawing lines only join up
/// when the rows nearly touch.
pub const LINE_HEIGHT: f32 = 1.35;

/// Where a key pressed while the terminal has focus goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRoute {
    /// To the shell, spelled as bytes.
    Shell,
    /// To the editor, as this action: the escape hatch.
    Editor(Action),
    /// Nowhere: a ⌘ key on a Mac that means nothing here, which must not reach the shell as the
    /// control character it would otherwise be.
    Nothing,
}

/// Which keys a focused terminal keeps for the editor. Everything else goes to the shell.
///
/// Deliberately few. A terminal that lets the editor's shortcuts through is one where `ctrl-w`
/// closes a buffer instead of deleting a word and `ctrl-p` opens a file picker instead of recalling
/// the last command — the shell eating keys, from the other side. So the editor keeps only what
/// cannot mean anything to a shell, or is how you leave:
///
/// - **`ctrl-``** toggles the terminal, so the key that brought you here takes you back.
/// - **`ctrl-t`** toggles it too, for the same reason: it is the other key that opens it. This
///   one does cost the shell something — readline's transpose-characters — and it is the one
///   trade on the list; a key that opens the panel and cannot close it would be a trap.
/// - **`ctrl-shift-c`** and **`ctrl-shift-v`** copy the mouse selection and paste. A shell has no
///   binding for ctrl-shift anything: a terminal sends the same control code with and without
///   shift, which is why GNOME Terminal, Konsole and VSCode all keep that family for themselves —
///   and why plain `ctrl-c` stays the shell's interrupt.
/// - **`ctrl-shift-p`** opens the command palette, from which everything else is reachable.
/// - **`ctrl-pageup`** and **`ctrl-pagedown`** step between terminal tabs, VSCode's keys for the
///   same thing. The second trade on the list: Vim binds them to its own tab pages, and inside a
///   Vim running here they now switch terminals instead. `gt` and `gT` still reach Vim's tabs.
///
/// Clicking the code is the other way out, and needs no key at all.
///
/// **The Keys page moves these too.** A key the user gave one of the commands above is kept for it
/// here, and a key of the list above whose command was given another is the shell's again — so
/// the terminal rows of the Keys page, which print [`dacode_input::shortcuts::TERMINAL_KEYS`], are
/// as true in the panel as the rest are in the code. Nothing else the user binds is taken from
/// the shell: that would be the shell eating keys from the editor's side.
pub fn route(key: Key) -> KeyRoute {
    let shortcuts = dacode_input::shortcuts::current();
    if let dacode_input::Match::Exact(action) = shortcuts.lookup(&[key]) {
        if TERMINAL_COMMANDS.contains(&action) {
            return KeyRoute::Editor(action);
        }
    }
    let route = built_in(key);
    match &route {
        KeyRoute::Editor(action) if shortcuts.suppresses(action) => KeyRoute::Shell,
        _ => route,
    }
}

/// Where a key with ⌘ held goes on a Mac, the key already carrying ctrl for it.
///
/// ⌘ is the Mac's own modifier and a shell never sees it, so nothing with ⌘ on it is the shell's:
/// ⌘C copies and ⌘V pastes, as they do in Terminal.app and iTerm — without that, ⌘C would arrive
/// as ctrl-c and interrupt whatever was running, which is the one thing a person copying output
/// is certain not to want. Everything else is what [`route`] would make of the ctrl key, and a key
/// it would have sent to the shell goes nowhere instead.
pub fn route_command(key: Key) -> KeyRoute {
    if !key.alt && !key.shift {
        match key.code {
            KeyCode::Char('c') => return KeyRoute::Editor(Action::TerminalCopy),
            KeyCode::Char('v') => return KeyRoute::Editor(Action::TerminalPaste),
            _ => {}
        }
    }
    match route(key) {
        KeyRoute::Shell => KeyRoute::Nothing,
        other => other,
    }
}

/// The commands a focused terminal keeps a key for.
const TERMINAL_COMMANDS: [Action; 9] = [
    Action::ToggleTerminal,
    Action::ZoomIn,
    Action::ZoomOut,
    Action::ZoomReset,
    Action::TerminalCopy,
    Action::TerminalPaste,
    Action::ToggleCommandPalette,
    Action::NextTerminal,
    Action::PreviousTerminal,
];

fn built_in(key: Key) -> KeyRoute {
    if !key.ctrl || key.alt {
        return KeyRoute::Shell;
    }
    let action = match key.code {
        KeyCode::Char('`') => Some(Action::ToggleTerminal),
        // Not with shift, which is the project's symbol picker and nothing a shell is owed.
        KeyCode::Char('t') if !key.shift => Some(Action::ToggleTerminal),
        KeyCode::Char('c') if key.shift => Some(Action::TerminalCopy),
        KeyCode::Char('v') if key.shift => Some(Action::TerminalPaste),
        KeyCode::Char('p') if key.shift => Some(Action::ToggleCommandPalette),
        KeyCode::PageDown if !key.shift => Some(Action::NextTerminal),
        KeyCode::PageUp if !key.shift => Some(Action::PreviousTerminal),
        // Zoom works wherever the keys are, or the person who needs it larger finds it stops
        // working the moment they click into the shell. No shell binds these: readline's undo is
        // `ctrl-_`, which arrives as a different key.
        KeyCode::Char('=') | KeyCode::Char('+') => Some(Action::ZoomIn),
        KeyCode::Char('-') if !key.shift => Some(Action::ZoomOut),
        KeyCode::Char('0') => Some(Action::ZoomReset),
        _ => None,
    };
    match action {
        Some(action) => KeyRoute::Editor(action),
        None => KeyRoute::Shell,
    }
}

/// The selection's background: the visual-mode accent washed into the panel, the same signal a
/// selection in the code gives.
pub fn selection_fill(theme: &Theme) -> Oklch {
    theme.bg_panel.mix(theme.accent(Mode::Visual), 0.32)
}

/// The colours a program asking `OSC 10`/`11`/`4` is told, which are the ones it is drawn in.
pub fn palette(theme: &Theme) -> dacode_terminal::Palette {
    let rgb = |color: Oklch| {
        let [r, g, b, _] = color.to_srgb8();
        [r, g, b]
    };
    let ansi = theme.ansi();
    dacode_terminal::Palette {
        foreground: rgb(theme.ink),
        background: rgb(theme.bg_panel),
        ansi: std::array::from_fn(|index| rgb(ansi[index])),
    }
}

/// A terminal colour in `theme`.
pub fn color(theme: &Theme, palette: &[Oklch; 16], color: Color) -> Oklch {
    match color {
        Color::Foreground => theme.ink,
        Color::Background => theme.bg_panel,
        Color::Ansi(index) => palette[usize::from(index.min(15))],
        Color::Rgb(r, g, b) => Oklch::from_srgb8(r, g, b),
    }
}

/// How one cell is painted: its glyph's colour, and a background to fill behind it when it is not
/// the panel's own.
///
/// One weight is bundled, so bold is shown the way terminals showed it before they had bold
/// faces: a named colour in its bright half. Dim mixes towards the background; inverse swaps.
pub fn paint(theme: &Theme, palette: &[Oklch; 16], cell: &Cell) -> (Oklch, Option<Oklch>) {
    let style = cell.style;
    let fg = match cell.fg {
        Color::Ansi(index) if style.bold && index < 8 => Color::Ansi(index + 8),
        other => other,
    };
    let (fg, bg) = if style.inverse {
        (cell.bg, fg)
    } else {
        (fg, cell.bg)
    };
    // Inverse always fills, even when the colour it fills with is the default: the default
    // foreground as a background is the whole point of it (a selection in `less`, a status bar).
    // The mouse selection fills over whatever the program asked for, as it does in every terminal.
    let fill = if cell.selected {
        Some(selection_fill(theme))
    } else {
        (style.inverse || bg != Color::Background).then(|| color(theme, palette, bg))
    };
    let mut ink = color(theme, palette, fg);
    if style.dim {
        ink = ink.mix(fill.unwrap_or(theme.bg_panel), 0.4);
    }
    (ink, fill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focused_terminal_keeps_every_editing_key_for_the_shell() {
        // The failure PLAN.md named: a terminal whose host eats keystrokes. These are the keys an
        // editor most wants and a shell most needs, and every one of them goes to the shell.
        for keys in [
            "<c-w>", "<c-p>", "<c-r>", "<c-c>", "<c-d>", "<c-s>", "<c-e>", "<c-a>", "<c-z>",
            "<c-k>", "<c-f>", "<esc>", "<tab>", "<cr>", "<f1>", "<c-b>", "<a-b>", "x", "<c-1>",
        ] {
            let key = Key::parse_sequence(keys)[0];
            assert_eq!(route(key), KeyRoute::Shell, "{keys} is the shell's");
        }
    }

    #[test]
    fn the_escape_hatch_is_seven_keys() {
        // `ctrl-t` among them: the editor's second key for the terminal must close it from inside
        // as well, or it opens a panel only the other key can leave.
        for (keys, action) in [
            ("<c-`>", Action::ToggleTerminal),
            ("<c-t>", Action::ToggleTerminal),
            ("<c-s-c>", Action::TerminalCopy),
            ("<c-s-v>", Action::TerminalPaste),
            ("<c-s-p>", Action::ToggleCommandPalette),
            ("<c-pagedown>", Action::NextTerminal),
            ("<c-pageup>", Action::PreviousTerminal),
        ] {
            let key = Key::parse_sequence(keys)[0];
            assert_eq!(
                route(key),
                KeyRoute::Editor(action),
                "{keys} is the editor's"
            );
        }
        // Without shift these are the shell's: ctrl-c interrupts, ctrl-v is "insert the next key
        // literally" and ctrl-p is the previous command. Without ctrl, page-up scrolls a pager.
        for keys in ["<c-c>", "<c-v>", "<c-p>", "<pageup>", "<s-pagedown>"] {
            let key = Key::parse_sequence(keys)[0];
            assert_eq!(route(key), KeyRoute::Shell, "{keys} is the shell's");
        }
    }

    #[test]
    fn command_keys_on_a_mac_copy_and_paste_and_never_reach_the_shell() {
        // ⌘C arrives carrying ctrl. Sent on as the shell's, it would be ctrl-c — an interrupt
        // where the person meant copy, killing the build whose output they were copying.
        dacode_input::shortcuts::install(dacode_input::Shortcuts::new());
        let key = |keys: &str| Key::parse_sequence(keys)[0];
        assert_eq!(
            route_command(key("<c-c>")),
            KeyRoute::Editor(Action::TerminalCopy)
        );
        assert_eq!(
            route_command(key("<c-v>")),
            KeyRoute::Editor(Action::TerminalPaste)
        );
        // The panel's own keys still work with ⌘ — ⌘` closes it, as ctrl-` does.
        assert_eq!(
            route_command(key("<c-`>")),
            KeyRoute::Editor(Action::ToggleTerminal)
        );
        for keys in ["<c-w>", "<c-d>", "<c-r>", "<c-z>"] {
            assert_eq!(route_command(key(keys)), KeyRoute::Nothing, "⌘ of {keys}");
        }
    }

    #[test]
    fn every_terminal_key_the_keys_page_prints_is_one_the_panel_answers() {
        // The command table's columns are tested against the grammar; these keys live outside it,
        // so this is what keeps the help panel and the Keys page from printing one that is not
        // the terminal's.
        dacode_input::shortcuts::install(dacode_input::Shortcuts::new());
        for (name, keys) in dacode_input::shortcuts::TERMINAL_KEYS {
            let key = Key::parse_sequence(keys)[0];
            let action = Action::from_name(name).expect("a command");
            assert_eq!(route(key), KeyRoute::Editor(action), "{keys} runs {name}");
        }
    }

    #[test]
    fn a_terminal_key_moved_on_the_keys_page_is_the_panels_and_the_old_one_the_shells() {
        let mut shortcuts = dacode_input::Shortcuts::new();
        shortcuts.assign(
            dacode_input::commands::by_name("next_terminal").unwrap(),
            Key::parse_sequence("<a-n>"),
        );
        dacode_input::shortcuts::install(shortcuts);
        assert_eq!(
            route(Key::parse_sequence("<a-n>")[0]),
            KeyRoute::Editor(Action::NextTerminal)
        );
        assert_eq!(
            route(Key::parse_sequence("<c-pagedown>")[0]),
            KeyRoute::Shell
        );
        dacode_input::shortcuts::install(dacode_input::Shortcuts::new());
    }

    #[test]
    fn bold_is_shown_as_brightness_and_inverse_swaps() {
        let theme = Theme::default();
        let palette = theme.ansi();
        let mut cell = Cell {
            fg: Color::Ansi(1),
            ..Cell::default()
        };
        cell.style.bold = true;
        let (fg, fill) = paint(&theme, &palette, &cell);
        assert_eq!(fg.to_hex(), palette[9].to_hex(), "bold red is bright red");
        assert_eq!(
            fill, None,
            "a default background is the panel's, not a fill"
        );

        let mut inverse = Cell::default();
        inverse.style.inverse = true;
        let (fg, fill) = paint(&theme, &palette, &inverse);
        assert_eq!(fg.to_hex(), theme.bg_panel.to_hex());
        assert_eq!(fill.map(|c| c.to_hex()), Some(theme.ink.to_hex()));
    }

    #[test]
    fn a_selected_cell_is_filled_whatever_the_program_painted_under_it() {
        // A selection over a coloured background — a `git diff`'s red, a status bar — has to read
        // as selected, or copying it is a guess at which cells are in.
        let theme = Theme::default();
        let palette = theme.ansi();
        let cell = Cell {
            bg: Color::Ansi(1),
            selected: true,
            ..Cell::default()
        };
        let (_, fill) = paint(&theme, &palette, &cell);
        assert_eq!(
            fill.map(|c| c.to_hex()),
            Some(selection_fill(&theme).to_hex())
        );
    }

    #[test]
    fn a_tab_is_named_by_its_shell_until_the_shell_names_it() {
        let shell = dacode_terminal::ShellCommand::new("/usr/bin/zsh", &[]);
        assert_eq!(tab_title(None, &shell), "zsh");
        assert_eq!(tab_title(Some("  ".into()), &shell), "zsh");
        assert_eq!(tab_title(Some("~/dacode".into()), &shell), "~/dacode");
        let windows = dacode_terminal::ShellCommand::new("pwsh.exe", &[]);
        assert_eq!(tab_title(None, &windows), "pwsh");
    }
}

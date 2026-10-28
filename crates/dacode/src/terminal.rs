//! The terminal panel, as the editor sees it: whether it is open, what it last showed, how its
//! cells are coloured, and which keys it keeps.
//!
//! The shell itself is not here. It is a process, and the executor owns no processes — the event
//! loop holds the [`dacode_terminal::Terminal`] beside the language servers, and hands this module a
//! [`Screen`] to draw. That split is also what lets the screenshot harness draw a terminal with no
//! shell behind it: it fills [`Panel::screen`] from an [`dacode_terminal::Emulator`] fed a fixed
//! transcript.

use dacode_input::{Key, KeyCode};
use dacode_terminal::{Cell, Color, Screen};
use dacode_theme::{Oklch, Theme};

/// The panel's state on [`crate::chrome::EditorState`].
#[derive(Debug, Clone, Default)]
pub struct Panel {
    /// Shown under the editor. Hiding it leaves the shell running, as VSCode does: closing a view
    /// is not a decision to kill whatever is running in it.
    pub open: bool,
    /// What the shell last showed, copied out once per frame that needed it. `None` before the
    /// first copy, which draws as an empty panel.
    pub screen: Option<Screen>,
    /// The title the shell set (`OSC 0`), shown in the panel's header.
    pub title: Option<String>,
}

/// The line height of a terminal row, in ems of the code size.
///
/// Tighter than the code's own generous 1.9: a terminal is read in blocks — a listing, a
/// backtrace, a table some program drew with box characters — and box-drawing lines only join up
/// when the rows nearly touch.
pub const LINE_HEIGHT: f32 = 1.35;

/// Where a key pressed while the terminal has focus goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    /// To the shell, spelled as bytes.
    Shell,
    /// To the editor, which runs it as it would anywhere: the escape hatch.
    Editor,
}

/// Which keys a focused terminal keeps for the editor. Everything else goes to the shell.
///
/// Deliberately four. A terminal that lets the editor's shortcuts through is one where `ctrl-w`
/// closes a buffer instead of deleting a word and `ctrl-p` opens a file picker instead of recalling
/// the last command — the shell eating keys, from the other side. So the editor keeps only what
/// cannot mean anything to a shell, or is how you leave:
///
/// - **`ctrl-``** toggles the terminal, so the key that brought you here takes you back.
/// - **`ctrl-t`** toggles it too, for the same reason: it is the other key that opens it. This
///   one does cost the shell something — readline's transpose-characters — and it is the one
///   trade on the list; a key that opens the panel and cannot close it would be a trap.
/// - **`ctrl-shift-v`** pastes. A shell has no binding for ctrl-shift anything: a terminal sends
///   the same control code with and without shift, which is why GNOME Terminal, Konsole and VSCode
///   all keep that family for themselves.
/// - **`ctrl-shift-p`** opens the command palette, from which everything else is reachable.
///
/// Clicking the code is the other way out, and needs no key at all.
pub fn route(key: Key) -> KeyRoute {
    let editor = key.ctrl
        && !key.alt
        && match key.code {
            KeyCode::Char('`') => true,
            // Not with shift, which is the project's symbol picker and nothing a shell is owed.
            KeyCode::Char('t') => !key.shift,
            KeyCode::Char('v') | KeyCode::Char('p') => key.shift,
            _ => false,
        };
    if editor {
        KeyRoute::Editor
    } else {
        KeyRoute::Shell
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
    let fill = (style.inverse || bg != Color::Background).then(|| color(theme, palette, bg));
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
    fn the_escape_hatch_is_four_keys() {
        // `ctrl-t` among them: the editor's second key for the terminal must close it from inside
        // as well, or it opens a panel only the other key can leave.
        for keys in ["<c-`>", "<c-t>", "<c-s-v>", "<c-s-p>"] {
            let key = Key::parse_sequence(keys)[0];
            assert_eq!(route(key), KeyRoute::Editor, "{keys} is the editor's");
        }
        // Without shift these are the shell's: ctrl-v is "insert the next key literally" and ctrl-p
        // is the previous command.
        for keys in ["<c-v>", "<c-p>"] {
            let key = Key::parse_sequence(keys)[0];
            assert_eq!(route(key), KeyRoute::Shell, "{keys} is the shell's");
        }
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
}

//! Keys as the bytes a shell expects.
//!
//! xterm's encoding, because `TERM=xterm-256color` is what the shell is told it is talking to and
//! every line editor — readline, zle, fish, PSReadLine — reads it. The editor's [`Key`] has already
//! been through `Key::from_platform`, so a character with ctrl held arrives lowercased with `shift`
//! beside it, and a character without a modifier arrives exactly as typed (`A` is a capital A).

use dacode_input::{Key, KeyCode};

/// What the terminal is doing that changes how a key is spelled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM, "application cursor keys": a full-screen program (`less`, `vim`, `htop`) asks for
    /// the arrows as `ESC O A` rather than `ESC [ A`, and they do nothing in it otherwise.
    pub app_cursor: bool,
}

/// The bytes for one key press, or `None` for a key that has no spelling in a terminal.
pub fn bytes_for(key: Key, modes: KeyModes) -> Option<Vec<u8>> {
    // The xterm modifier parameter: 1 plus a bit each for shift, alt and ctrl.
    let modifier = 1 + u8::from(key.shift) + 2 * u8::from(key.alt) + 4 * u8::from(key.ctrl);
    let modified = modifier > 1;

    let csi = |final_byte: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |number: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[{number};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{number}~").into_bytes()
        }
    };

    let bytes = match key.code {
        KeyCode::Char(c) => return character(c, key.ctrl, key.alt),
        KeyCode::Enter => alt_prefixed(key.alt, b"\r"),
        // DEL, not BS: every Unix terminal since the VT220 sends 0x7f for the key labelled
        // backspace, and readline binds `^H` to something else in some configurations.
        KeyCode::Backspace if key.ctrl => alt_prefixed(key.alt, b"\x08"),
        KeyCode::Backspace => alt_prefixed(key.alt, b"\x7f"),
        KeyCode::Tab if key.shift => b"\x1b[Z".to_vec(),
        KeyCode::Tab => alt_prefixed(key.alt, b"\t"),
        KeyCode::Escape => alt_prefixed(key.alt, b"\x1b"),
        KeyCode::Up => csi('A'),
        KeyCode::Down => csi('B'),
        KeyCode::Right => csi('C'),
        KeyCode::Left => csi('D'),
        KeyCode::Home => csi('H'),
        KeyCode::End => csi('F'),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::Function(number @ 1..=4) => {
            let final_byte = (b'P' + number - 1) as char;
            if modified {
                format!("\x1b[1;{modifier}{final_byte}").into_bytes()
            } else {
                format!("\x1bO{final_byte}").into_bytes()
            }
        }
        KeyCode::Function(number) => {
            let code = match number {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return None,
            };
            tilde(code)
        }
    };
    Some(bytes)
}

fn alt_prefixed(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

/// A character, with ctrl as a control code and alt as an escape in front — the "meta sends
/// escape" convention every Linux terminal defaults to, and the one readline's `alt-b`/`alt-f`
/// word motions are written against.
fn character(c: char, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    let body = if ctrl {
        let code = match c {
            'a'..='z' => c as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '/' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            // No control code to send: dropped rather than typed as the plain character, which
            // would put text on the command line that nobody typed.
            _ => return None,
        };
        vec![code]
    } else {
        let mut buffer = [0u8; 4];
        c.encode_utf8(&mut buffer).as_bytes().to_vec()
    };
    Some(alt_prefixed(alt, &body))
}

/// Text from the clipboard, as the bytes a paste should send.
///
/// Line endings become `\r`, which is what the Enter key sends — a pasted `\n` reaches a shell as
/// `^J`, and some line editors do not treat it as the end of a line. With bracketed paste on (the
/// shell asked for it with `ESC [ ? 2004 h`), the text is wrapped so the shell can tell it was
/// pasted rather than typed and need not run each line as it arrives; any end-of-paste marker inside
/// the text is removed first, or a crafted clipboard could end the paste early and have the rest
/// run as if typed.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalised = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let clean = normalised.replace("\x1b[201~", "");
        let mut out = Vec::with_capacity(clean.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalised.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(keys: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for key in Key::parse_sequence(keys) {
            out.extend(bytes_for(key, KeyModes::default()).expect("a spelling"));
        }
        out
    }

    #[test]
    fn printable_characters_are_themselves() {
        assert_eq!(bytes("ls -la"), b"ls -la");
        assert_eq!(bytes("é"), "é".as_bytes(), "UTF-8, not Latin-1");
        // Shift is already applied by the window system, so a capital is just a capital.
        let capital = Key::from_platform(KeyCode::Char('A'), false, false, true);
        assert_eq!(bytes_for(capital, KeyModes::default()).unwrap(), b"A");
    }

    #[test]
    fn the_editing_keys_are_what_a_line_editor_expects() {
        assert_eq!(bytes("<cr>"), b"\r");
        assert_eq!(bytes("<bs>"), b"\x7f");
        assert_eq!(bytes("<tab>"), b"\t");
        assert_eq!(bytes("<s-tab>"), b"\x1b[Z");
        assert_eq!(bytes("<esc>"), b"\x1b");
        assert_eq!(bytes("<del>"), b"\x1b[3~");
    }

    #[test]
    fn the_arrows_follow_application_cursor_mode() {
        // Without DECCKM a shell gets CSI; `less` and `vim` turn it on and get SS3. Sending the
        // wrong one is an arrow key that does nothing inside a pager.
        assert_eq!(bytes("<up><down><right><left>"), b"\x1b[A\x1b[B\x1b[C\x1b[D");
        let app = KeyModes { app_cursor: true };
        assert_eq!(
            bytes_for(Key::named(KeyCode::Up), app).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(bytes("<home><end>"), b"\x1b[H\x1b[F");
        assert_eq!(bytes("<pageup><pagedown>"), b"\x1b[5~\x1b[6~");
    }

    #[test]
    fn modified_navigation_keys_carry_the_xterm_parameter() {
        // ctrl-← is word-left in every shell, and it only is because the modifier arrives.
        assert_eq!(bytes("<c-left>"), b"\x1b[1;5D");
        assert_eq!(bytes("<s-up>"), b"\x1b[1;2A");
        assert_eq!(bytes("<a-right>"), b"\x1b[1;3C");
        assert_eq!(bytes("<c-pageup>"), b"\x1b[5;5~");
    }

    #[test]
    fn ctrl_letters_are_control_codes() {
        assert_eq!(bytes("<c-c>"), [3]);
        assert_eq!(bytes("<c-d>"), [4]);
        assert_eq!(bytes("<c-a><c-e>"), [1, 5]);
        assert_eq!(bytes("<c-z>"), [26]);
        assert_eq!(bytes("<c-[>"), [0x1b]);
        assert_eq!(bytes("<c-space>"), [0]);
    }

    #[test]
    fn alt_is_an_escape_in_front() {
        assert_eq!(bytes("<a-b>"), b"\x1bb");
        assert_eq!(bytes("<a-f>"), b"\x1bf");
        assert_eq!(bytes("<c-a-h>"), [0x1b, 8]);
        assert_eq!(bytes("<a-bs>"), b"\x1b\x7f");
    }

    #[test]
    fn function_keys_are_spelled_as_xterm_spells_them() {
        assert_eq!(bytes("<f1>"), b"\x1bOP");
        assert_eq!(bytes("<f5>"), b"\x1b[15~");
        assert_eq!(bytes("<f12>"), b"\x1b[24~");
    }

    #[test]
    fn a_ctrl_chord_with_no_control_code_sends_nothing() {
        // Better nothing than the bare character: ctrl-; typing a `;` would be text nobody typed.
        assert_eq!(bytes_for(Key::ctrl(';'), KeyModes::default()), None);
    }

    #[test]
    fn a_paste_ends_its_lines_as_enter_does() {
        assert_eq!(paste("a\nb\r\nc", false), b"a\rb\rc");
    }

    #[test]
    fn a_bracketed_paste_cannot_be_ended_from_inside() {
        // A clipboard holding the end marker would otherwise close the bracket early, and what
        // followed it would run as though typed — `rm -rf` included.
        let pasted = paste("safe\x1b[201~rm -rf ~\n", true);
        assert_eq!(pasted, b"\x1b[200~saferm -rf ~\r\x1b[201~");
    }
}

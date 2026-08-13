//! Key representation.
//!
//! Deliberately independent of `winit`: the grammar is driven from tests, from macros and from
//! Lua config, none of which have a window. The binary translates platform events into these.

use std::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Char(char),
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// `F1`–`F12`. A named key and not a character: a function key is not text, and one that fell
    /// through to `Char` would be typed into the document. They exist because the conventional
    /// bindings for rename, go-to-definition and help are function keys everywhere else, and
    /// someone who turned the modal grammar off is telling you they expect the conventional ones.
    Function(u8),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    /// Held shift. Only consulted where it changes meaning rather than the character produced —
    /// non-modal selection with the arrow keys, which is the one gesture every editor shares.
    pub shift: bool,
}

impl Key {
    pub fn char(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub fn named(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    /// The same key with shift held.
    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// Parses a key sequence for tests and config: `"3dw"`, `"<esc>"`, `"<c-r>"`.
    ///
    /// Existing because a grammar is only testable if a test can express `d2f)` as literally
    /// that, rather than as a dozen struct literals.
    pub fn parse_sequence(input: &str) -> Vec<Key> {
        let mut keys = Vec::new();
        let chars: Vec<char> = input.chars().collect();
        let mut index = 0;

        while index < chars.len() {
            if chars[index] == '<' {
                if let Some(close) = chars[index..].iter().position(|c| *c == '>') {
                    let name: String = chars[index + 1..index + close].iter().collect();
                    if let Some(key) = Self::parse_named(&name) {
                        keys.push(key);
                        index += close + 1;
                        continue;
                    }
                }
            }
            keys.push(Key::char(chars[index]));
            index += 1;
        }

        keys
    }

    fn parse_named(name: &str) -> Option<Key> {
        let lower = name.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("c-") {
            let c = rest.chars().next()?;
            return Some(Key::ctrl(c));
        }
        if let Some(rest) = lower.strip_prefix("a-") {
            let c = rest.chars().next()?;
            return Some(Key {
                code: KeyCode::Char(c),
                ctrl: false,
                alt: true,
                shift: false,
            });
        }

        // `<f1>` … `<f12>`, before the table: they are the one family with a number in the name.
        if let Some(number) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            if (1..=12).contains(&number) {
                return Some(Key::named(KeyCode::Function(number)));
            }
        }

        let code = match lower.as_str() {
            "esc" | "escape" => KeyCode::Escape,
            "cr" | "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "bs" | "backspace" => KeyCode::Backspace,
            "del" | "delete" => KeyCode::Delete,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            _ => return None,
        };
        Some(Key::named(code))
    }

    /// How the key appears in the statusline's pending display.
    pub fn to_display(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "<space>".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Escape => "<esc>".into(),
            KeyCode::Enter => "<cr>".into(),
            KeyCode::Tab => "<tab>".into(),
            KeyCode::Backspace => "<bs>".into(),
            KeyCode::Delete => "<del>".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Home => "<home>".into(),
            KeyCode::End => "<end>".into(),
            KeyCode::PageUp => "<pgup>".into(),
            KeyCode::PageDown => "<pgdn>".into(),
            KeyCode::Function(number) => format!("<f{number}>"),
        };

        // Shift is shown only for the keys where it is not already in the character: `<s-f12>` is
        // meaningful, `<s-a>` would be a confusing way to write `A`.
        let shifted = self.shift && !matches!(self.code, KeyCode::Char(_));
        match (self.ctrl, self.alt, shifted) {
            (true, true, _) => format!("<c-a-{base}>"),
            (true, _, true) => format!("<c-s-{base}>"),
            (true, _, _) => format!("<c-{base}>"),
            (_, true, true) => format!("<a-s-{base}>"),
            (_, true, _) => format!("<a-{base}>"),
            (_, _, true) => format!("<s-{base}>"),
            _ => base,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display())
    }
}

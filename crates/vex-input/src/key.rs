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
        };

        match (self.ctrl, self.alt) {
            (true, _) => format!("<c-{base}>"),
            (_, true) => format!("<a-{base}>"),
            _ => base,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display())
    }
}

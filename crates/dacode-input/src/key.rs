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

    /// Whether the key is a *shortcut* rather than part of the text or of Vim's grammar: something
    /// held with ctrl or alt, or a function key.
    ///
    /// The line the Keys page draws. A shortcut means the same thing in every mode, so it can be
    /// handed to another command without touching anything else; a letter, an arrow or `Enter` is
    /// typing, or a word of Vim's language, and taking one away would break the language rather than
    /// move a key.
    pub fn is_shortcut(self) -> bool {
        self.ctrl || self.alt || matches!(self.code, KeyCode::Function(_))
    }

    /// Builds a key from what the window system reported, normalising the character.
    ///
    /// **A character with ctrl or alt held is a binding, not text.** Window systems apply shift to
    /// the character before handing it over, so `ctrl-shift-p` arrives as `P` — and every binding
    /// written as `Char('p') if shift` silently never matched. That is exactly what happened:
    /// `ctrl-shift-p` opened nothing, and `ctrl-shift-k`, `ctrl-shift-l`, `ctrl-shift-z` and
    /// `ctrl-shift-f` were dead with it. The shift is not lost — it is in `shift`, where a binding
    /// can read it deliberately instead of by accident of letter case.
    ///
    /// Without a modifier the character is left exactly as it came: `A` is a letter someone typed,
    /// and lowercasing it would insert the wrong text.
    pub fn from_platform(code: KeyCode, ctrl: bool, alt: bool, shift: bool) -> Self {
        let code = match code {
            KeyCode::Char(c) if ctrl || alt => {
                // `to_lowercase` yields a sequence for a few characters; a binding is one char, and
                // anything that is not is left alone rather than mangled.
                let mut lowered = c.to_lowercase();
                match (lowered.next(), lowered.next()) {
                    (Some(single), None) => KeyCode::Char(single),
                    _ => KeyCode::Char(c),
                }
            }
            other => other,
        };
        Self {
            code,
            ctrl,
            alt,
            shift,
        }
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
            // The arrow glyphs, which `to_display` writes bare and the cheatsheet is full of.
            let code = match chars[index] {
                '←' => Some(KeyCode::Left),
                '→' => Some(KeyCode::Right),
                '↑' => Some(KeyCode::Up),
                '↓' => Some(KeyCode::Down),
                _ => None,
            };
            keys.push(match code {
                Some(code) => Key::named(code),
                None => Key::char(chars[index]),
            });
            index += 1;
        }

        keys
    }

    fn parse_named(name: &str) -> Option<Key> {
        let mut lower = name.to_ascii_lowercase();

        // Modifiers, in any combination and any order: `<c-s-k>`, `<a-s-down>`, `<c-a-p>`. Stripped
        // in a loop rather than matched as pairs, because the combinations multiply and the loop is
        // the same three lines however many there are. This has to round-trip `to_display`, or the
        // command table cannot be checked against the grammar — which is what makes the help panel
        // and the palette trustworthy rather than hopeful.
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        loop {
            let stripped = match () {
                _ if lower.starts_with("c-") => {
                    ctrl = true;
                    lower.split_off(2)
                }
                _ if lower.starts_with("a-") => {
                    alt = true;
                    lower.split_off(2)
                }
                _ if lower.starts_with("s-") => {
                    shift = true;
                    lower.split_off(2)
                }
                _ => break,
            };
            lower = stripped;
        }

        // `<f1>` … `<f12>`, before the table: they are the one family with a number in the name.
        if let Some(number) = lower
            .strip_prefix('f')
            .and_then(|rest| rest.parse::<u8>().ok())
            .filter(|number| (1..=12).contains(number))
        {
            return Some(Key {
                code: KeyCode::Function(number),
                ctrl,
                alt,
                shift,
            });
        }

        // A single character with modifiers on it: `<c-s>`, `<a-x>`, and the arrow glyphs the
        // cheatsheet and the command table are written with.
        if lower.chars().count() == 1 {
            let c = lower.chars().next()?;
            let code = match c {
                '←' => KeyCode::Left,
                '→' => KeyCode::Right,
                '↑' => KeyCode::Up,
                '↓' => KeyCode::Down,
                _ => KeyCode::Char(c),
            };
            return Some(Key {
                code,
                ctrl,
                alt,
                shift,
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
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            _ => return None,
        };
        Some(Key {
            code,
            ctrl,
            alt,
            shift,
        })
    }

    /// How the key appears in the statusline's pending display, the cheatsheet and the command table.
    ///
    /// The name is written *without* its brackets and wrapped exactly once at the end. Building it
    /// with them and then wrapping again produced `<s-<tab>>`, which nothing can read back — the
    /// round-trip test found it, and it had been latent in `<c-<tab>>` since the day this was
    /// written, because nobody had bound a modified named key before.
    pub fn to_display(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Escape => "esc".into(),
            KeyCode::Enter => "cr".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Backspace => "bs".into(),
            KeyCode::Delete => "del".into(),
            // The arrows keep their glyphs: they read better in a statusline hint, and
            // `parse_sequence` takes them bare.
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pgup".into(),
            KeyCode::PageDown => "pgdn".into(),
            KeyCode::Function(number) => format!("f{number}"),
        };
        let arrow = matches!(
            self.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
        );

        // Shift is shown unless it is already in the character. `<s-a>` would be a confusing way to
        // write `A` — but `<c-s-k>` is *not* `<c-K>`: with another modifier held, shift is part of
        // the binding and dropping it would silently merge two different keys into one.
        let plain_char = matches!(self.code, KeyCode::Char(c) if c != ' ');
        let shifted = self.shift && (!plain_char || self.ctrl || self.alt);
        let modifiers = match (self.ctrl, self.alt, shifted) {
            (true, true, true) => "c-a-s-",
            (true, true, false) => "c-a-",
            (true, false, true) => "c-s-",
            (true, false, false) => "c-",
            (false, true, true) => "a-s-",
            (false, true, false) => "a-",
            (false, false, true) => "s-",
            (false, false, false) => "",
        };

        if modifiers.is_empty() && (arrow || plain_char) {
            return base;
        }
        format!("<{modifiers}{base}>")
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_survives_being_written_down_and_read_back() {
        // `to_display` writes the keys the help panel, the cheatsheet and the command table are
        // spelled with; `parse_sequence` reads them. If the two disagree, every check that feeds the
        // documented keys through the real grammar silently checks the wrong key — which is worse
        // than no check, because it passes.
        let keys = [
            Key::ctrl('s'),
            Key::char('x'),
            Key::named(KeyCode::Escape),
            Key::named(KeyCode::Tab).with_shift(),
            Key::named(KeyCode::Function(12)).with_shift(),
            Key {
                code: KeyCode::Up,
                ctrl: false,
                alt: true,
                shift: true,
            },
            Key {
                code: KeyCode::Char('k'),
                ctrl: true,
                alt: false,
                shift: true,
            },
            Key {
                code: KeyCode::Backspace,
                ctrl: true,
                alt: false,
                shift: false,
            },
        ];

        for key in keys {
            let written = key.to_display();
            let read = Key::parse_sequence(&written);
            assert_eq!(read.len(), 1, "{written} did not read back as one key");
            assert_eq!(read[0], key, "{written} read back as something else");
        }
    }

    #[test]
    fn a_sequence_is_read_as_its_keys() {
        assert_eq!(Key::parse_sequence("3dw").len(), 3);
        assert_eq!(
            Key::parse_sequence("<c-w>v"),
            vec![Key::ctrl('w'), Key::char('v')]
        );
    }

    #[test]
    fn a_function_key_is_not_text() {
        // The bug this prevents: F2 falling through to `Char` and being typed into the file.
        let key = Key::parse_sequence("<f2>");
        assert_eq!(key[0].code, KeyCode::Function(2));
        assert!(Key::parse_sequence("<f13>")[0].code != KeyCode::Function(13));
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn a_modified_letter_arrives_as_a_binding_not_as_typed_text() {
        // The bug this exists for: window systems apply shift to the character, so `ctrl-shift-p`
        // is reported as `P`. Every binding written as `Char('p') if shift` then misses, which is
        // how the command palette, delete-line, select-all-occurrences, redo and project search all
        // shipped dead in the same release.
        let key = Key::from_platform(KeyCode::Char('P'), true, false, true);
        assert_eq!(key.code, KeyCode::Char('p'), "lowercased for matching");
        assert!(key.shift, "and the shift is still there to match on");
    }

    #[test]
    fn an_unmodified_capital_is_left_alone() {
        // `A` with no modifier is a letter someone typed. Lowercasing it would insert `a`, which is
        // a far worse bug than the one above.
        let key = Key::from_platform(KeyCode::Char('A'), false, false, true);
        assert_eq!(key.code, KeyCode::Char('A'));
    }

    #[test]
    fn alt_counts_as_a_modifier_too() {
        let key = Key::from_platform(KeyCode::Char('D'), false, true, true);
        assert_eq!(key.code, KeyCode::Char('d'));
    }

    #[test]
    fn a_named_key_passes_through_untouched() {
        let key = Key::from_platform(KeyCode::Function(12), false, false, true);
        assert_eq!(key.code, KeyCode::Function(12));
        assert!(key.shift);
    }
}

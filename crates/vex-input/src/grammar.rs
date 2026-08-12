//! The modal state machine.
//!
//! Parses Vim's grammar:
//!
//! ```text
//! [count] ["register] operator [count] {motion | text-object}
//! ```
//!
//! Modelled as an explicit machine that accumulates a *pending* prefix and emits an
//! [`Action`] the moment the prefix resolves. Counts and registers are first-class prefixes
//! rather than flags bolted on afterwards — retrofitting them is how a Vim clone ends up with
//! `3dw` working and `d3w` not.
//!
//! Helix-style selection-first commands coexist here: they are separate keys, not a second
//! grammar, so both paths produce the same Actions.

use crate::action::{Action, InsertPosition, Motion, Operator, Target, TextObject};
use crate::key::{Key, KeyCode};
use vex_theme::Mode;

/// What the machine did with a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The key resolved into something to do.
    Action(Action),
    /// The key was consumed as part of an incomplete command. The string is what the statusline
    /// shows, so the user can see the machine's state rather than guessing at it.
    Pending(String),
    /// Not bound in this mode. Deliberately distinct from `Pending` so a stray key clears the
    /// prefix rather than silently accumulating.
    Unhandled,
}

/// What the machine is waiting for next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Nothing pending.
    Start,
    /// Saw `"`, waiting for the register name.
    Register,
    /// Saw an operator, waiting for a motion or text object.
    Target,
    /// Saw `i` or `a` in target position, waiting for the object kind.
    ObjectKind { around: bool },
    /// Saw `f`/`F`/`t`/`T`, waiting for the character to search for.
    FindChar { forward: bool, till: bool },
    /// Saw `r`, waiting for the replacement character.
    ReplaceChar,
    /// Saw `g`, waiting for the second key (`gg`, `ge`, …).
    GPrefix,
    /// Saw `]` or `[`, waiting for what to jump to.
    BracketPrefix { forward: bool },
}

#[derive(Debug, Clone)]
pub struct Grammar {
    stage: Stage,
    count: Option<usize>,
    /// A count typed *after* the operator — `d3w`. Multiplies with the first.
    operator_count: Option<usize>,
    register: Option<char>,
    operator: Option<Operator>,
    /// Raw keys of the pending command, for display.
    display: String,
}

impl Default for Grammar {
    fn default() -> Self {
        Self::new()
    }
}

impl Grammar {
    pub fn new() -> Self {
        Self {
            stage: Stage::Start,
            count: None,
            operator_count: None,
            register: None,
            operator: None,
            display: String::new(),
        }
    }

    /// What the statusline shows: the partially-typed command.
    pub fn pending(&self) -> &str {
        &self.display
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Start && self.display.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// The effective count: `2d3w` deletes 6 words, as in Vim.
    fn total_count(&self) -> usize {
        self.count.unwrap_or(1) * self.operator_count.unwrap_or(1)
    }

    fn emit(&mut self, action: Action) -> Resolution {
        self.clear();
        Resolution::Action(action)
    }

    fn push_display(&mut self, key: &Key) {
        self.display.push_str(&key.to_display());
    }

    /// Feeds one key press. `mode` is the current editor mode.
    pub fn feed(&mut self, mode: Mode, key: Key) -> Resolution {
        // Escape always wins, in every mode and at every stage: it is the escape hatch, and an
        // editor where a half-typed command can trap you is unusable.
        if key.code == KeyCode::Escape {
            let was_pending = !self.is_idle();
            self.clear();
            return if was_pending {
                // Cancel the command, stay in the mode.
                Resolution::Pending(String::new())
            } else {
                Resolution::Action(Action::CollapseSelections)
            };
        }

        match mode {
            Mode::Insert | Mode::Replace => self.feed_insert(mode, key),
            Mode::Normal | Mode::Visual => self.feed_normal(mode, key),
        }
    }

    /// Insert and replace mode: text goes in, with no grammar to parse.
    fn feed_insert(&mut self, mode: Mode, key: Key) -> Resolution {
        if let Some(action) = self.editing_key(key) {
            return Resolution::Action(action);
        }

        match key.code {
            KeyCode::Char(c) if !key.ctrl && !key.alt => {
                if mode == Mode::Replace {
                    // Overwrite rather than insert; the delete and the insert are one intent.
                    Resolution::Action(Action::ReplaceChar(c))
                } else {
                    Resolution::Action(Action::InsertText(c.to_string()))
                }
            }
            KeyCode::Enter => Resolution::Action(Action::Newline),
            KeyCode::Tab => Resolution::Action(Action::InsertText("    ".into())),
            KeyCode::Backspace => Resolution::Action(Action::DeleteBackward),
            KeyCode::Delete => Resolution::Action(Action::DeleteForward),
            _ => Resolution::Unhandled,
        }
    }

    /// Keys that mean the same thing regardless of mode: arrows, page keys, Ctrl bindings.
    fn editing_key(&mut self, key: Key) -> Option<Action> {
        if key.ctrl {
            return match key.code {
                KeyCode::Char('r') => Some(Action::Redo),
                KeyCode::Char('s') => Some(Action::Save),
                KeyCode::Char('q') => Some(Action::Quit),
                _ => None,
            };
        }

        let motion = match key.code {
            KeyCode::Left => Motion::Left,
            KeyCode::Right => Motion::Right,
            KeyCode::Up => Motion::Up,
            KeyCode::Down => Motion::Down,
            KeyCode::Home => Motion::LineStart,
            KeyCode::End => Motion::LineEnd,
            _ => return None,
        };

        Some(Action::Move { motion, count: 1 })
    }

    fn feed_normal(&mut self, mode: Mode, key: Key) -> Resolution {
        let extend = mode == Mode::Visual;

        // Arrow keys and Ctrl bindings bypass the grammar, but must respect visual mode.
        if let Some(action) = self.editing_key(key) {
            let action = match (action, extend) {
                (Action::Move { motion, count }, true) => Action::Extend { motion, count },
                (action, _) => action,
            };
            return self.emit(action);
        }

        match self.stage {
            Stage::Register => return self.take_register(key),
            Stage::FindChar { forward, till } => {
                return self.take_find_char(key, forward, till, extend);
            }
            Stage::ReplaceChar => return self.take_replace_char(key),
            Stage::ObjectKind { around } => return self.take_object_kind(key, around),
            Stage::GPrefix => return self.take_g_prefix(key, extend),
            Stage::BracketPrefix { forward } => return self.take_bracket_prefix(key, forward),
            Stage::Start | Stage::Target => {}
        }

        let KeyCode::Char(c) = key.code else {
            return Resolution::Unhandled;
        };

        // Counts. A leading `0` is the line-start motion, not a count digit — the one place
        // where digit handling has to know what came before it.
        if c.is_ascii_digit()
            && !(c == '0' && self.count.is_none() && self.operator_count.is_none())
        {
            let digit = c.to_digit(10).unwrap() as usize;
            let slot = if self.operator.is_some() {
                &mut self.operator_count
            } else {
                &mut self.count
            };
            *slot = Some(slot.unwrap_or(0) * 10 + digit);
            self.push_display(&key);
            return Resolution::Pending(self.display.clone());
        }

        if c == '"' && self.register.is_none() {
            self.stage = Stage::Register;
            self.push_display(&key);
            return Resolution::Pending(self.display.clone());
        }

        // In target position, `i`/`a` introduce a text object rather than insert mode.
        if self.stage == Stage::Target && (c == 'i' || c == 'a') {
            self.stage = Stage::ObjectKind { around: c == 'a' };
            self.push_display(&key);
            return Resolution::Pending(self.display.clone());
        }

        // A doubled operator is the linewise form: `dd`, `yy`, `cc`.
        if let Some(operator) = self.operator {
            if operator_key(c) == Some(operator) {
                let count = self.total_count();
                let register = self.register;
                return self.emit(Action::Apply {
                    operator,
                    target: Target::WholeLine { count },
                    register,
                });
            }
        }

        // Motions resolve immediately, or complete a pending operator.
        if let Some(motion) = self.motion_key(c) {
            return self.resolve_motion(motion, extend);
        }

        match c {
            'f' | 'F' | 't' | 'T' => {
                self.stage = Stage::FindChar {
                    forward: c == 'f' || c == 't',
                    till: c == 't' || c == 'T',
                };
                self.push_display(&key);
                Resolution::Pending(self.display.clone())
            }

            'g' => {
                self.stage = Stage::GPrefix;
                self.push_display(&key);
                Resolution::Pending(self.display.clone())
            }

            ']' | '[' => {
                self.stage = Stage::BracketPrefix { forward: c == ']' };
                self.push_display(&key);
                Resolution::Pending(self.display.clone())
            }

            // Operators. In visual mode they apply to the selection immediately.
            _ if operator_key(c).is_some() => {
                let operator = operator_key(c).unwrap();
                if extend {
                    let register = self.register;
                    return self.emit(Action::Apply {
                        operator,
                        target: Target::Selection,
                        register,
                    });
                }
                if self.operator.is_some() {
                    // A second, different operator is not a command; drop the prefix.
                    self.clear();
                    return Resolution::Unhandled;
                }
                self.operator = Some(operator);
                self.stage = Stage::Target;
                self.push_display(&key);
                Resolution::Pending(self.display.clone())
            }

            // Everything below is only valid with no operator pending.
            _ if self.operator.is_some() => {
                self.clear();
                Resolution::Unhandled
            }

            'i' => self.emit(Action::InsertAt(InsertPosition::BeforeCursor)),
            'a' => self.emit(Action::InsertAt(InsertPosition::AfterCursor)),
            'I' => self.emit(Action::InsertAt(InsertPosition::LineStart)),
            'A' => self.emit(Action::InsertAt(InsertPosition::LineEnd)),
            'o' => self.emit(Action::InsertAt(InsertPosition::LineBelow)),
            'O' => self.emit(Action::InsertAt(InsertPosition::LineAbove)),
            'v' => self.emit(Action::EnterMode(Mode::Visual)),
            'R' => self.emit(Action::EnterMode(Mode::Replace)),

            'x' => {
                let count = self.total_count();
                let register = self.register;
                self.emit(Action::Apply {
                    operator: Operator::Delete,
                    target: if extend {
                        Target::Selection
                    } else {
                        Target::Motion {
                            motion: Motion::Right,
                            count,
                        }
                    },
                    register,
                })
            }
            'r' => {
                self.stage = Stage::ReplaceChar;
                self.push_display(&key);
                Resolution::Pending(self.display.clone())
            }

            'p' | 'P' => {
                let count = self.total_count();
                let register = self.register;
                self.emit(Action::Paste {
                    register,
                    before: c == 'P',
                    count,
                })
            }

            '/' => self.emit(Action::StartSearch { forward: true }),
            '?' => self.emit(Action::StartSearch { forward: false }),
            'n' => self.emit(Action::RepeatSearch { forward: true }),
            'N' => self.emit(Action::RepeatSearch { forward: false }),
            ':' => self.emit(Action::StartCommand),

            'u' => self.emit(Action::Undo),
            '.' => self.emit(Action::RepeatChange),

            // Multi-cursor, Helix-style but on Vim's grammar.
            'C' => self.emit(Action::AddCaretBelow),
            // `K` for hover: the convention every Neovim LSP configuration uses.
            'K' => self.emit(Action::Hover),
            'S' => self.emit(Action::SplitSelectionIntoLines),
            ',' => self.emit(Action::KeepPrimarySelection),
            'V' => {
                let count = self.total_count();
                self.emit(Action::SelectLine { count })
            }

            'z' => self.emit(Action::ToggleSoftWrap),
            '=' | '+' => self.emit(Action::IncreaseFontSize),
            '-' => self.emit(Action::DecreaseFontSize),

            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }

    /// Turns a resolved motion into either a move or an operator application.
    fn resolve_motion(&mut self, motion: Motion, extend: bool) -> Resolution {
        let count = self.total_count();

        if let Some(operator) = self.operator {
            let register = self.register;
            return self.emit(Action::Apply {
                operator,
                target: Target::Motion { motion, count },
                register,
            });
        }

        if extend {
            self.emit(Action::Extend { motion, count })
        } else {
            self.emit(Action::Move { motion, count })
        }
    }

    fn motion_key(&self, c: char) -> Option<Motion> {
        Some(match c {
            'h' => Motion::Left,
            'l' | ' ' => Motion::Right,
            'j' => Motion::Down,
            'k' => Motion::Up,
            'w' => Motion::WordForward,
            'W' => Motion::WordForwardBig,
            'b' => Motion::WordBackward,
            'B' => Motion::WordBackwardBig,
            'e' => Motion::WordEnd,
            'E' => Motion::WordEndBig,
            '0' => Motion::LineStart,
            '$' => Motion::LineEnd,
            '^' => Motion::FirstNonBlank,
            '%' => Motion::MatchingBracket,
            '{' => Motion::ParagraphBackward,
            '}' => Motion::ParagraphForward,
            'G' => {
                // `G` with a count goes to that line; without one, to the end of the file.
                return Some(match self.count {
                    Some(line) => Motion::ToLine(line.saturating_sub(1)),
                    None => Motion::FileEnd,
                });
            }
            _ => return None,
        })
    }

    fn take_register(&mut self, key: Key) -> Resolution {
        if let KeyCode::Char(name) = key.code {
            self.register = Some(name);
            self.stage = if self.operator.is_some() {
                Stage::Target
            } else {
                Stage::Start
            };
            self.push_display(&key);
            Resolution::Pending(self.display.clone())
        } else {
            self.clear();
            Resolution::Unhandled
        }
    }

    fn take_find_char(&mut self, key: Key, forward: bool, till: bool, extend: bool) -> Resolution {
        if let KeyCode::Char(target) = key.code {
            self.stage = if self.operator.is_some() {
                Stage::Target
            } else {
                Stage::Start
            };
            self.resolve_motion(
                Motion::FindChar {
                    target,
                    forward,
                    till,
                },
                extend,
            )
        } else {
            self.clear();
            Resolution::Unhandled
        }
    }

    fn take_replace_char(&mut self, key: Key) -> Resolution {
        if let KeyCode::Char(c) = key.code {
            self.emit(Action::ReplaceChar(c))
        } else {
            self.clear();
            Resolution::Unhandled
        }
    }

    fn take_object_kind(&mut self, key: Key, around: bool) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        let object = match c {
            'w' => TextObject::Word { around, big: false },
            'W' => TextObject::Word { around, big: true },
            '"' | '\'' | '`' => TextObject::Quote {
                delimiter: c,
                around,
            },
            '(' | ')' | 'b' => TextObject::Pair {
                open: '(',
                close: ')',
                around,
            },
            '[' | ']' => TextObject::Pair {
                open: '[',
                close: ']',
                around,
            },
            '{' | '}' | 'B' => TextObject::Pair {
                open: '{',
                close: '}',
                around,
            },
            '<' | '>' => TextObject::Pair {
                open: '<',
                close: '>',
                around,
            },
            'p' => TextObject::Paragraph { around },
            'l' => TextObject::Line { around },
            _ => {
                self.clear();
                return Resolution::Unhandled;
            }
        };

        // A text object without an operator selects it — the Helix-style path into the same
        // Action the operator path uses.
        let Some(operator) = self.operator else {
            self.clear();
            return Resolution::Action(Action::Apply {
                operator: Operator::Yank,
                target: Target::Object(object),
                register: None,
            });
        };

        let register = self.register;
        self.emit(Action::Apply {
            operator,
            target: Target::Object(object),
            register,
        })
    }

    fn take_g_prefix(&mut self, key: Key, extend: bool) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        match c {
            'g' => {
                // `gg` goes to the first line, or to the count-th line if one was typed.
                let motion = match self.count {
                    Some(line) => Motion::ToLine(line.saturating_sub(1)),
                    None => Motion::FileStart,
                };
                self.stage = Stage::Start;
                self.resolve_motion(motion, extend)
            }
            'e' => {
                self.stage = Stage::Start;
                self.resolve_motion(Motion::FileEnd, extend)
            }
            'h' => {
                self.stage = Stage::Start;
                self.resolve_motion(Motion::LineStart, extend)
            }
            'l' => {
                self.stage = Stage::Start;
                self.resolve_motion(Motion::LineEnd, extend)
            }
            'd' => self.emit(Action::GotoDefinition),
            't' => self.emit(Action::ToggleTheme),
            'a' => self.emit(Action::SelectAll),
            'A' => self.emit(Action::AddCaretAbove),
            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }
}

impl Grammar {
    fn take_bracket_prefix(&mut self, key: Key, forward: bool) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        match c {
            // `]d` / `[d`, as LSP configurations spell it.
            'd' => self.emit(Action::NextDiagnostic { forward }),
            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }
}

fn operator_key(c: char) -> Option<Operator> {
    Some(match c {
        'd' => Operator::Delete,
        'c' => Operator::Change,
        'y' => Operator::Yank,
        '>' => Operator::Indent,
        '<' => Operator::Outdent,
        'U' => Operator::Uppercase,
        'L' => Operator::Lowercase,
        _ => return None,
    })
}

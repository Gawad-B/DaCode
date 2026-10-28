//! `dacode-input` — keys into [`Action`]s.
//!
//! Three pieces, each testable on its own:
//!
//! - [`action`] — the vocabulary of everything the editor can do. Every keymap, command-palette
//!   entry, macro and dot-repeat resolves to one of these values.
//! - [`grammar`] — the modal state machine that parses
//!   `[count]["register]operator[count]{motion|text-object}`.
//! - [`resolve`] — motions and text objects evaluated against real text.
//!
//! Vertical motion is the one deliberate hole: it depends on soft-wrap geometry, so the caller
//! routes `Motion::Up`/`Down` to `dacode-display` instead of resolving it here.

pub mod action;
pub mod commands;
pub mod grammar;
pub mod history;
pub mod key;
pub mod keymap;
pub mod resolve;

pub use action::{Action, InsertPosition, Motion, Operator, SyntaxObject, Target, TextObject};
pub use commands::{COMMANDS, Command, Group};
pub use grammar::{Grammar, Resolution};
pub use history::{Jump, JumpList, Macros, is_jump};
pub use key::{Key, KeyCode};
pub use keymap::{BindError, Keymap, Match};
pub use resolve::{line_range, operator_range, resolve_motion, resolve_object};

use std::collections::HashMap;

/// Register storage: the unnamed register plus named ones.
///
/// A yank without a register name goes to the unnamed register, which is what `p` reads — the
/// behavior that makes `dw` then `p` a transposition rather than a surprise.
#[derive(Debug, Default, Clone)]
pub struct Registers {
    unnamed: String,
    named: HashMap<char, String>,
}

impl Registers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, register: Option<char>, contents: String) {
        match register {
            Some(name) => {
                self.named.insert(name, contents.clone());
                // Vim also updates the unnamed register on a named yank.
                self.unnamed = contents;
            }
            None => self.unnamed = contents,
        }
    }

    pub fn get(&self, register: Option<char>) -> &str {
        match register {
            Some(name) => self.named.get(&name).map(String::as_str).unwrap_or(""),
            None => &self.unnamed,
        }
    }

    /// Whether the register holds whole lines, which decides whether `p` pastes above the
    /// cursor or inline.
    pub fn is_linewise(&self, register: Option<char>) -> bool {
        self.get(register).ends_with('\n')
    }
}

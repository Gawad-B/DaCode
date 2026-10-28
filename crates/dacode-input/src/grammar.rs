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

use crate::action::{Action, InsertPosition, Motion, Operator, SyntaxObject, Target, TextObject};
use crate::key::{Key, KeyCode};
use crate::keymap::{Keymap, Match};
use dacode_theme::Mode;

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
    /// Saw `ctrl-w`, waiting for a window command.
    WindowPrefix,
    /// Saw `z`, waiting for a fold command.
    FoldPrefix,
    /// Saw `q`, waiting for the register to record into.
    RecordRegister,
    /// Saw `@`, waiting for the register to replay.
    ReplayRegister,
    /// Saw `ctrl-k`, waiting for the second key of a chord.
    ///
    /// VSCode's convention rather than Vim's, because the things behind it — open folder, settings —
    /// are the ones people already reach for with `ctrl-k` and have no Vim spelling to inherit.
    ChordPrefix,
}

#[derive(Debug, Clone)]
pub struct Grammar {
    /// Whether Vim's modal grammar is in force.
    ///
    /// With it off the editor behaves the way every non-modal editor does: letters always insert, and
    /// the shortcuts are the ones on the menus everywhere else — `ctrl-c`, `ctrl-v`, `ctrl-z`. The
    /// grammar is not *removed*, it is bypassed: everything still resolves to the same `Action`
    /// vocabulary, so a command works identically whichever way it was reached.
    modal: bool,
    /// User bindings, consulted before the built-ins while idle. Empty unless config sets any.
    keymap: Keymap,
    /// Keys collected while a user binding is still only a prefix.
    user_keys: Vec<Key>,
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
            modal: true,
            keymap: Keymap::new(),
            user_keys: Vec::new(),
            stage: Stage::Start,
            count: None,
            operator_count: None,
            register: None,
            operator: None,
            display: String::new(),
        }
    }

    /// Turns Vim's grammar on or off. Off means letters insert and modes do not apply.
    pub fn set_modal(&mut self, modal: bool) {
        self.modal = modal;
        self.clear();
    }

    pub fn is_modal(&self) -> bool {
        self.modal
    }

    /// Installs user bindings. Replaces any previously set, so a config reload cannot leave a
    /// binding behind that the file no longer contains.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// What the statusline shows: the partially-typed command.
    pub fn pending(&self) -> &str {
        &self.display
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Start && self.display.is_empty()
    }

    pub fn clear(&mut self) {
        // Not `*self = Self::new()`: that would discard the user's keymap on every resolved
        // command, silently unbinding their config after the first keystroke.
        self.stage = Stage::Start;
        self.count = None;
        self.operator_count = None;
        self.register = None;
        self.operator = None;
        self.display.clear();
        self.user_keys.clear();
    }

    /// The effective count: `2d3w` deletes 6 words, as in Vim.
    fn total_count(&self) -> usize {
        self.count.unwrap_or(1) * self.operator_count.unwrap_or(1)
    }

    /// The line `G` or `gg` was aimed at, if any count was typed at all.
    ///
    /// **Either count**, which is the whole point. A count typed *after* an operator lands in
    /// `operator_count`, so `G` reading only `count` saw nothing for `d5G` and fell through to "the
    /// end of the file" — deleting from the cursor to the last line instead of to line five. The
    /// same for `d5gg` upwards. A count is not the same question as whether one was given, so this
    /// returns an `Option` rather than defaulting to 1 and losing the distinction.
    fn target_line(&self) -> Option<usize> {
        (self.count.is_some() || self.operator_count.is_some()).then(|| self.total_count())
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

        // Function keys are the editor's, not the grammar's: F2 renames and F12 goes to a
        // definition in both modes, the way they do in every other editor. Checked only when
        // nothing is half-typed, so they cannot land in the middle of `d3` or `f`.
        if self.is_idle() {
            if let Some(action) = function_key(key) {
                return self.emit(action);
            }
        }

        // Without the modal grammar there is one behavior, not four: text goes in, and the
        // non-modal shortcuts do the rest. `Mode::Insert` is passed along because that is what the
        // rest of the editor reads to mean "typing inserts".
        if !self.modal {
            return self.feed_flat(key);
        }

        match mode {
            Mode::Insert | Mode::Replace => self.feed_insert(mode, key),
            Mode::Normal | Mode::Visual => self.feed_normal(mode, key),
        }
    }

    /// Key handling with the modal grammar switched off.
    ///
    /// The shortcuts are the ones every other editor has, because that is the entire point of the
    /// switch: someone who turns Vim off is telling you they expect `ctrl-c` to copy. Selection is
    /// still a set and the actions are still the same actions — only the way in differs.
    ///
    /// This was thirteen keys once — copy, paste, undo, save and the arrows — and that was the
    /// whole non-modal editor. It read as the modal one with the good parts deleted, which is
    /// exactly what it was: Vim reaches "move this line down" by composing `ddp`, and with the
    /// grammar off there is nothing to compose with. Every line below is a verb Vim never needed
    /// to name. The vocabulary is VSCode's rather than invented, because muscle memory is not a
    /// thing an editor gets to have opinions about.
    fn feed_flat(&mut self, key: Key) -> Resolution {
        // A chord already in progress owns the key. Without this the second press of `ctrl-w v` is
        // taken as text and types a `v` into the document.
        match self.stage {
            Stage::WindowPrefix => return self.take_window_prefix(key),
            Stage::ChordPrefix => return self.take_chord(key),
            _ => {}
        }

        // Alt with the vertical arrows is line surgery, and with the horizontal ones it is the
        // jumplist — the back button every editor with a "go to definition" needs.
        if key.alt && !key.ctrl {
            let action = match key.code {
                KeyCode::Up if key.shift => Some(Action::DuplicateLine { down: false }),
                KeyCode::Down if key.shift => Some(Action::DuplicateLine { down: true }),
                // VSCode's "expand selection". Before the plain `alt-→` below, which is the
                // jumplist and would otherwise take the shifted key too.
                KeyCode::Right if key.shift => Some(Action::ExpandSelection),
                KeyCode::Up => Some(Action::MoveLine { down: false }),
                KeyCode::Down => Some(Action::MoveLine { down: true }),
                KeyCode::Left => Some(Action::Jump { forward: false }),
                KeyCode::Right => Some(Action::Jump { forward: true }),
                // The structural selections. VSCode has no default for these, and alt with a
                // letter was typing the letter, so nothing anyone relied on moves.
                KeyCode::Char('f') => Some(Action::select_syntax(SyntaxObject::Function)),
                KeyCode::Char('t') => Some(Action::select_syntax(SyntaxObject::Type)),
                KeyCode::Char('a') => Some(Action::select_syntax(SyntaxObject::Argument)),
                // GitLens's key for file blame. Before the plain `b`: a shifted binding written
                // after the unshifted one never matches.
                KeyCode::Char('b') if key.shift => Some(Action::ShowCommit),
                KeyCode::Char('b') => Some(Action::ToggleBlame),
                _ => None,
            };
            if let Some(action) = action {
                return self.emit(action);
            }
        }

        if key.ctrl {
            let action = match key.code {
                KeyCode::Char('c') => Some(Action::Apply {
                    operator: Operator::Yank,
                    target: Target::Selection,
                    register: None,
                }),
                KeyCode::Char('x') => Some(Action::Apply {
                    operator: Operator::Delete,
                    target: Target::Selection,
                    register: None,
                }),
                KeyCode::Char('v') => Some(Action::Paste {
                    register: None,
                    before: false,
                    count: 1,
                }),
                // ctrl-y and ctrl-shift-z both redo: Windows learned the first, everywhere else the
                // second, and someone who tries the wrong one is not making a mistake worth punishing.
                KeyCode::Char('z') if key.shift => Some(Action::Redo),
                KeyCode::Char('z') => Some(Action::Undo),
                KeyCode::Char('y') => Some(Action::Redo),
                KeyCode::Char('a') => Some(Action::SelectAll),
                KeyCode::Char('s') if key.shift => Some(Action::SaveAs),
                KeyCode::Char('s') => Some(Action::Save),
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Char('f') if key.shift => Some(Action::SearchProject),
                KeyCode::Char('f') => Some(Action::StartSearch { forward: true }),
                KeyCode::Char('h') => Some(Action::StartReplace),
                KeyCode::Char('g') => Some(Action::StartCommand),
                // `ctrl-d` is occurrence selection, not "a cursor below": it is the first shortcut a
                // VSCode user reaches for, and having it do something else is worse than not having
                // it. The cursor-below gesture is ctrl-alt-↑/↓, which is where VSCode keeps it.
                KeyCode::Char('d') => Some(Action::SelectNextOccurrence { all: false }),
                KeyCode::Char('l') if key.shift => Some(Action::SelectNextOccurrence { all: true }),
                KeyCode::Char('l') => Some(Action::SelectLine { count: 1 }),
                KeyCode::Char('/') => Some(Action::ToggleComment),
                KeyCode::Char('k') if key.shift => Some(Action::Apply {
                    operator: Operator::Delete,
                    target: Target::WholeLine { count: 1 },
                    register: None,
                }),
                KeyCode::Char('p') if key.shift => Some(Action::ToggleCommandPalette),
                // Before the plain `ctrl-e` in `feed_editor_chords`, which is the tree's toggle.
                KeyCode::Char('e') if key.shift => Some(Action::FocusExplorer),
                KeyCode::Char('1') => Some(Action::FocusEditor),
                // VSCode's key for its terminal, and free here: nothing else uses the backtick
                // with ctrl, and Vim's own backtick (a mark jump) is never chorded.
                KeyCode::Char('`') => Some(Action::ToggleTerminal),
                KeyCode::Char(' ') => Some(Action::RequestCompletion),
                KeyCode::Backspace => Some(Action::DeleteWord { forward: false }),
                KeyCode::Delete => Some(Action::DeleteWord { forward: true }),
                // Word-wise motion, as ctrl-arrow does everywhere, and extending with shift.
                KeyCode::Left => Some(motion_or_extend(Motion::WordBackward, key.shift)),
                KeyCode::Right => Some(motion_or_extend(Motion::WordForward, key.shift)),
                KeyCode::Home => Some(motion_or_extend(Motion::FileStart, key.shift)),
                KeyCode::End => Some(motion_or_extend(Motion::FileEnd, key.shift)),
                KeyCode::Up if key.alt => Some(Action::AddCaretAbove),
                KeyCode::Down if key.alt => Some(Action::AddCaretBelow),
                _ => None,
            };
            if let Some(action) = action {
                return self.emit(action);
            }
            // `ctrl-w`, `ctrl-k`, `ctrl-p` and the rest keep working: they are editor commands, not
            // Vim ones, and turning the grammar off should not cost you the file tree.
            return self.feed_editor_chords(key);
        }

        // Shift with an arrow extends the selection, which is the one gesture every editor shares.
        let motion = match key.code {
            KeyCode::Left => Some(Motion::Left),
            KeyCode::Right => Some(Motion::Right),
            KeyCode::Up => Some(Motion::Up),
            KeyCode::Down => Some(Motion::Down),
            KeyCode::Home => Some(Motion::LineStart),
            KeyCode::End => Some(Motion::LineEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            return self.emit(motion_or_extend(motion, key.shift));
        }

        match key.code {
            KeyCode::Char(c) => self.emit(Action::InsertText(c.to_string())),
            KeyCode::Enter => self.emit(Action::Newline),
            // Shift-tab is always outdent. Plain tab expands a snippet or indents, which is what
            // `ExpandSnippet` already falls back to — so a selection spanning lines still indents.
            KeyCode::Tab if key.shift => self.emit(Action::Apply {
                operator: Operator::Outdent,
                target: Target::Selection,
                register: None,
            }),
            KeyCode::Tab => self.emit(Action::ExpandSnippet),
            KeyCode::Backspace => self.emit(Action::DeleteBackward),
            KeyCode::Delete => self.emit(Action::DeleteForward),
            KeyCode::Escape => self.emit(Action::DismissInfo),
            // Paging and the arrows are handled above; anything left is for the application.
            _ => Resolution::Unhandled,
        }
    }

    /// The editor's own Ctrl chords, which are not Vim's and survive the grammar being off.
    fn feed_editor_chords(&mut self, key: Key) -> Resolution {
        match key.code {
            // Both push what was typed before returning `Pending`. Without it the which-key popup
            // and the statusline were handed an empty string: with the grammar off, `ctrl-w` and
            // `ctrl-k` waited for a second key and said nothing at all about it — and those two are
            // *only* reachable this way when Vim is off, so the popup that exists to teach them
            // never appeared for the people who needed it.
            KeyCode::Char('w') => {
                self.push_display(&key);
                self.stage = Stage::WindowPrefix;
                Resolution::Pending(self.display.clone())
            }
            KeyCode::Char('k') => {
                self.push_display(&key);
                self.stage = Stage::ChordPrefix;
                Resolution::Pending(self.display.clone())
            }
            KeyCode::Char('e') => self.emit(Action::ToggleTree),
            // Before the unshifted `o`, which is jump-back: a shifted binding written after the
            // plain one never matches, and that is exactly how five bindings shipped dead in 0.2.0.
            KeyCode::Char('o') if key.shift => self.emit(Action::GotoSymbol),
            KeyCode::Char('o') => self.emit(Action::Jump { forward: false }),
            KeyCode::Char('t') => self.emit(Action::GotoSymbolInProject),
            KeyCode::Char('i') => self.emit(Action::Jump { forward: true }),
            KeyCode::Char(',') => self.emit(Action::ToggleSettings),
            _ => Resolution::Unhandled,
        }
    }

    /// Insert and replace mode: text goes in, with no grammar to parse.
    fn feed_insert(&mut self, mode: Mode, key: Key) -> Resolution {
        // Bindings apply here too, but only to keys that could not be text: a multi-key binding on
        // a letter would have to hold that letter back to see what follows, which means typing
        // stutters. Modified and named keys have no such cost.
        let bindable = key.ctrl || key.alt || !matches!(key.code, KeyCode::Char(_));
        if bindable && !self.keymap.is_empty() {
            if let Match::Exact(action) = self.keymap.lookup(mode, &[key]) {
                return Resolution::Action(action);
            }
        }

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
            // Tab tries the snippet first and falls back to an indent, so it stays the indent key
            // for everyone who has no snippets.
            KeyCode::Tab => Resolution::Action(Action::ExpandSnippet),
            KeyCode::Backspace => Resolution::Action(Action::DeleteBackward),
            KeyCode::Delete => Resolution::Action(Action::DeleteForward),
            _ => Resolution::Unhandled,
        }
    }

    /// Keys that mean the same thing regardless of mode: arrows, page keys, Ctrl bindings.
    fn editing_key(&mut self, key: Key) -> Option<Action> {
        if key.ctrl {
            return match key.code {
                // Works in insert mode too, which is where it is actually used.
                KeyCode::Char(' ') => Some(Action::RequestCompletion),
                KeyCode::Char('r') => Some(Action::Redo),
                KeyCode::Char('s') => Some(Action::Save),
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Char('e') if key.shift => Some(Action::FocusExplorer),
                KeyCode::Char('e') => Some(Action::ToggleTree),
                KeyCode::Char('1') => Some(Action::FocusEditor),
                KeyCode::Char('`') => Some(Action::ToggleTerminal),
                // In both grammars. Vim's own answer is `:s/a/b/g`, which is a fine thing to know
                // and a poor thing to be the only way — and the prompt it opens is the same one, so
                // there is no second implementation to keep in step.
                KeyCode::Char('h') => Some(Action::StartReplace),
                // Shifted first, or the plain arm swallows it — see `feed_editor_chords`.
                KeyCode::Char('o') if key.shift => Some(Action::GotoSymbol),
                KeyCode::Char('o') => Some(Action::Jump { forward: false }),
                KeyCode::Char('t') => Some(Action::GotoSymbolInProject),
                KeyCode::Char('i') => Some(Action::Jump { forward: true }),
                KeyCode::Char(',') => Some(Action::ToggleSettings),
                // The palette of every command works in both grammars. Vim's own vocabulary has no
                // way to ask "what can this editor do?", and inventing a normal-mode key for it
                // would put the answer behind the same problem it solves.
                KeyCode::Char('p') if key.shift => Some(Action::ToggleCommandPalette),
                // Extra cursors, where VSCode keeps them. Modal users have `s` and visual mode, but
                // nobody arrives knowing that, and this costs nothing.
                KeyCode::Up if key.alt => Some(Action::AddCaretAbove),
                KeyCode::Down if key.alt => Some(Action::AddCaretBelow),
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
        if let Some(resolution) = self.feed_user_binding(mode, key) {
            return resolution;
        }
        self.feed_builtin(mode, key)
    }

    /// Tries the user's bindings, and only while the grammar is idle — a binding that fired with an
    /// operator or count pending could leave the grammar mid-command.
    ///
    /// Returns `None` to mean "not mine, use the built-ins".
    fn feed_user_binding(&mut self, mode: Mode, key: Key) -> Option<Resolution> {
        if self.keymap.is_empty() || (self.user_keys.is_empty() && !self.is_idle()) {
            return None;
        }

        self.user_keys.push(key);
        match self.keymap.lookup(mode, &self.user_keys) {
            Match::Exact(action) => Some(self.emit(action)),
            Match::Prefix => {
                self.push_display(&key);
                Some(Resolution::Pending(self.display.clone()))
            }
            // The sequence died. Replay it through the built-ins, so a partial match of a user
            // binding does not swallow keys that mean something on their own.
            Match::None => {
                let replay = std::mem::take(&mut self.user_keys);
                self.display.clear();

                let mut last = Resolution::Unhandled;
                for key in replay {
                    last = self.feed_builtin(mode, key);
                    // A resolved action mid-replay is returned as-is; anything after it belongs to
                    // the next command and would need queueing the caller cannot express. Bindings
                    // whose prefix is itself a complete command are the user's own ambiguity.
                    if matches!(last, Resolution::Action(_)) {
                        break;
                    }
                }
                Some(last)
            }
        }
    }

    /// Hands the key to whatever the grammar is already waiting for, or `None` if it is waiting
    /// for nothing a second key completes.
    ///
    /// One definition, consulted twice: once above the global Ctrl bindings for the two prefixes a
    /// modifier can still be held through, and once below for everything else. Writing the match
    /// out at both sites is how the two would drift.
    fn take_pending(&mut self, key: Key, extend: bool) -> Option<Resolution> {
        Some(match self.stage {
            Stage::Register => self.take_register(key),
            Stage::FindChar { forward, till } => self.take_find_char(key, forward, till, extend),
            Stage::ReplaceChar => self.take_replace_char(key),
            Stage::ObjectKind { around } => self.take_object_kind(key, around),
            Stage::GPrefix => self.take_g_prefix(key, extend),
            Stage::BracketPrefix { forward } => self.take_bracket_prefix(key, forward),
            Stage::WindowPrefix => self.take_window_prefix(key),
            Stage::FoldPrefix => self.take_fold_prefix(key),
            Stage::RecordRegister => self.take_macro_register(key, true),
            Stage::ReplayRegister => self.take_macro_register(key, false),
            Stage::ChordPrefix => self.take_chord(key),
            // `Target` waits for a motion, which is the rest of `feed_builtin` rather than a taker.
            Stage::Start | Stage::Target => return None,
        })
    }

    fn feed_builtin(&mut self, mode: Mode, key: Key) -> Resolution {
        let extend = mode == Mode::Visual;

        // `ctrl-w` is a prefix, so it is claimed before the single-key Ctrl bindings below.
        if key.ctrl && key.code == KeyCode::Char('w') && self.stage == Stage::Start {
            self.stage = Stage::WindowPrefix;
            self.push_display(&key);
            return Resolution::Pending(self.display.clone());
        }

        // `ctrl-k` likewise, for the VSCode-style chords.
        if key.ctrl && key.code == KeyCode::Char('k') && self.stage == Stage::Start {
            self.stage = Stage::ChordPrefix;
            self.push_display(&key);
            return Resolution::Pending(self.display.clone());
        }

        // A chord entered *with ctrl held* claims its second key before the global Ctrl bindings
        // below get a chance at it. A VSCode-style chord is typed without letting go of ctrl —
        // that is what makes it a chord — so `ctrl-k ctrl-s` reached `editing_key` as a plain
        // `ctrl-s`, saved the file and dropped the prefix, and `ctrl-k ctrl-o` jumped backwards
        // instead of opening a folder. Only these two stages, because they are the only ones a
        // modifier can still be down for; the rest wait on an ordinary character, and reordering
        // them would change what an arrow key does mid-command for no reported reason.
        if matches!(self.stage, Stage::WindowPrefix | Stage::ChordPrefix) {
            if let Some(resolution) = self.take_pending(key, extend) {
                return resolution;
            }
        }

        // Arrow keys and Ctrl bindings bypass the grammar, but must respect visual mode.
        if let Some(action) = self.editing_key(key) {
            let action = match (action, extend) {
                (Action::Move { motion, count }, true) => Action::Extend { motion, count },
                (action, _) => action,
            };
            return self.emit(action);
        }

        if let Some(resolution) = self.take_pending(key, extend) {
            return resolution;
        }

        // A Ctrl key the grammar does not know is *not* a character. Falling through here made
        // `ctrl-p` paste and `ctrl-b` move a word back, which only looked harmless because the
        // application intercepts both before the grammar sees them.
        //
        // Both of these clear first. They are only reached at `Start` or `Target`, but a count, a
        // register or an operator can be live at either — and leaving one behind is invisible, so
        // the next key acts on it. The application clears *its own* copy of the pending string on
        // `Unhandled` (main.rs), which is what the statusline and the which-key popup read: the
        // display went blank while the grammar was still holding `d3`, and the next `w` deleted
        // three words.
        if key.ctrl || key.alt {
            self.clear();
            return Resolution::Unhandled;
        }

        let KeyCode::Char(c) = key.code else {
            self.clear();
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

        // In target position, `i`/`a` introduce a text object rather than insert mode — and in
        // visual mode too, where `vaf` selects the function, as it does in Vim.
        let object_position = self.stage == Stage::Target
            || (extend && self.stage == Stage::Start && self.operator.is_none());
        if object_position && (c == 'i' || c == 'a') {
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
            'q' => {
                self.push_display(&key);
                self.stage = Stage::RecordRegister;
                Resolution::Pending(self.display.clone())
            }
            '@' => {
                self.push_display(&key);
                self.stage = Stage::ReplayRegister;
                Resolution::Pending(self.display.clone())
            }
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

            'z' => {
                // A prefix, as in Vim. Soft wrap moves to `zw`: `za`/`zR`/`zM` are muscle memory
                // worth matching, and a single-key `z` would make them unreachable.
                self.push_display(&key);
                self.stage = Stage::FoldPrefix;
                Resolution::Pending(self.display.clone())
            }
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
                return Some(match self.target_line() {
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
            // From the syntax tree. Vim's own `t` is the XML tag object, which this grammar never
            // had; `f`, `a` and `c` were free too.
            'f' => TextObject::Syntax {
                kind: SyntaxObject::Function,
                around,
            },
            't' => TextObject::Syntax {
                kind: SyntaxObject::Type,
                around,
            },
            'a' => TextObject::Syntax {
                kind: SyntaxObject::Argument,
                around,
            },
            'c' => TextObject::Syntax {
                kind: SyntaxObject::Comment,
                around,
            },
            _ => {
                self.clear();
                return Resolution::Unhandled;
            }
        };

        // A text object without an operator — visual mode's `vaf` — selects it.
        let Some(operator) = self.operator else {
            self.clear();
            return Resolution::Action(Action::SelectObject(object));
        };

        let register = self.register;
        self.emit(Action::Apply {
            operator,
            target: Target::Object(object),
            register,
        })
    }

    /// The second key of a `ctrl-k` chord. Accepted with or without Ctrl held, because nobody lets go
    /// of it deliberately between the two presses.
    fn take_chord(&mut self, key: Key) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        match c {
            'o' => self.emit(Action::OpenFolder),
            's' => self.emit(Action::ToggleSettings),
            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }

    /// The register after `q` or `@`.
    fn take_macro_register(&mut self, key: Key, record: bool) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        if record {
            return self.emit(Action::RecordMacro { register: c });
        }
        // `@@` repeats the last register, which is what the `None` means.
        self.emit(Action::ReplayMacro {
            register: (c != '@').then_some(c),
        })
    }

    /// `z` and then a fold command.
    fn take_fold_prefix(&mut self, key: Key) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        match c {
            'a' | 'c' | 'o' => self.emit(Action::ToggleFold),
            'R' => self.emit(Action::FoldAll { close: false }),
            'M' => self.emit(Action::FoldAll { close: true }),
            // DaCode's own: soft wrap has no Vim spelling to collide with.
            'w' => self.emit(Action::ToggleSoftWrap),
            // Vim's, but eight columns a press rather than one — one is unusable on a screen 200
            // columns wide, and the cheat sheet says which it is.
            'h' => self.emit(Action::ScrollHorizontally { right: false }),
            'l' => self.emit(Action::ScrollHorizontally { right: true }),
            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }

    /// `ctrl-w` and then a window command, as Vim spells it.
    ///
    /// Panes are a flat row or column rather than a tree, so `h`/`l` and `j`/`k` both mean "the
    /// neighbour that way" — which in a single-axis layout is simply the previous or next pane.
    fn take_window_prefix(&mut self, key: Key) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        match c {
            'v' => self.emit(Action::SplitPane { vertical: true }),
            's' => self.emit(Action::SplitPane { vertical: false }),
            'c' | 'q' => self.emit(Action::ClosePane),
            // Backwards on shift as well as on `W`: someone holding ctrl through the whole chord
            // sends `ctrl-shift-w`, which arrives lowercased with shift set rather than as `W`.
            'w' if key.shift => self.emit(Action::FocusPane { forward: false }),
            'w' | 'l' | 'j' => self.emit(Action::FocusPane { forward: true }),
            'W' | 'h' | 'k' => self.emit(Action::FocusPane { forward: false }),
            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }

    fn take_g_prefix(&mut self, key: Key, extend: bool) -> Resolution {
        let KeyCode::Char(c) = key.code else {
            self.clear();
            return Resolution::Unhandled;
        };

        match c {
            'g' => {
                // `gg` goes to the first line, or to the count-th line if one was typed.
                let motion = match self.target_line() {
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
            // `gr`, as Neovim 0.11 binds it. `gq` would be the Vim spelling for formatting, but that
            // is reflow-a-paragraph in Vim and this is reformat-the-file; a different key avoids
            // teaching the wrong reflex.
            'r' => self.emit(Action::Rename),
            'f' => self.emit(Action::Format),
            // `ga` was select-all; code actions are asked for far more often, and select-all moves
            // to `gA`'s neighbourhood as `g%` — the whole file, in the same spirit as `%`.
            'a' => self.emit(Action::CodeAction),
            'u' => self.emit(Action::FindReferences),
            's' => self.emit(Action::GotoSymbol),
            'S' => self.emit(Action::GotoSymbolInProject),
            'i' => self.emit(Action::ToggleInlayHints),
            // Blame: `b` for the column, and the shifted key for the one commit under the caret —
            // the same pairing as `]c` and `]C`. Vim leaves both free.
            'b' => self.emit(Action::ToggleBlame),
            'B' => self.emit(Action::ShowCommit),
            'D' => self.emit(Action::ToggleDiagnosticsList),
            '%' => self.emit(Action::SelectAll),
            't' => self.emit(Action::ToggleTheme),
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
            // `]c` / `[c` is what Vim itself uses for the next change in a diff, and what every
            // git-gutter plugin adopted from it.
            'c' => self.emit(Action::GotoChange { forward }),
            // `]C` for the destructive one: a shifted neighbour of the key that walks to it, so
            // undoing the change you just stepped onto is one key away and not a key you can hit
            // by accident while walking.
            'C' => self.emit(Action::RevertHunk),
            _ => {
                self.clear();
                Resolution::Unhandled
            }
        }
    }
}

/// What a function key does, in either grammar.
///
/// These are the bindings that are the same in VSCode, Visual Studio, IntelliJ and Eclipse, which
/// makes them the closest thing this industry has to a standard: F2 renames, F12 goes to the
/// definition, F8 walks the errors. Someone who has used any editor knows these without being told,
/// and that is the whole argument for spending four keys on them.
///
/// F1 is help rather than the command palette — VSCode's F1 — on purpose. A user who is lost presses
/// F1, and a panel listing every key that works right now answers the question they actually have,
/// including where the command palette is. The reverse is not true: a palette answers nothing if you
/// do not yet know what to search for.
fn function_key(key: Key) -> Option<Action> {
    let KeyCode::Function(number) = key.code else {
        return None;
    };

    // ctrl-F4 closes the file, as it does in every tabbed application on Windows. `ctrl-w` is the
    // other muscle memory for this and cannot have it: it is already the pane prefix, in both
    // grammars. The tab's own close button is there for everyone who would guess neither.
    if key.ctrl && number == 4 {
        return Some(Action::CloseBuffer);
    }

    Some(match (number, key.shift) {
        (1, _) => Action::ToggleHelp,
        (2, _) => Action::Rename,
        (3, false) => Action::RepeatSearch { forward: true },
        (3, true) => Action::RepeatSearch { forward: false },
        // The two things you step through in a file: what is wrong with it, and what you changed.
        // Neighbouring keys because they are the same gesture pointed at different marks.
        (7, false) => Action::GotoChange { forward: true },
        (7, true) => Action::GotoChange { forward: false },
        (8, false) => Action::NextDiagnostic { forward: true },
        (8, true) => Action::NextDiagnostic { forward: false },
        (10, false) => Action::ToggleMenu,
        (10, true) => Action::ToggleContextMenu,
        (12, false) => Action::GotoDefinition,
        (12, true) => Action::FindReferences,
        _ => return None,
    })
}

/// A motion, or the same motion extending the selection when shift is held. Every arrow and
/// word-motion binding needs this pair, and writing it out twice is how one of them ends up not
/// extending.
fn motion_or_extend(motion: Motion, shift: bool) -> Action {
    if shift {
        Action::Extend { motion, count: 1 }
    } else {
        Action::Move { motion, count: 1 }
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

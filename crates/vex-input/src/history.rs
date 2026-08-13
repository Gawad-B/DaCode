//! The jumplist and macro recording.
//!
//! Both are about *replaying what you did* rather than about the text, which is why they live beside
//! the grammar rather than in the buffer. And both are only possible because every user-visible
//! operation is an [`Action`](crate::Action): a macro is a list of Actions, and a jump is one that
//! declared itself far-reaching. Neither needed a new mechanism — which is the payoff the plan
//! predicted for making the vocabulary explicit before writing the code that produces it.

use crate::action::{Action, Motion};

/// Where the cursor was, and in which file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jump {
    /// `None` for a scratch buffer, which cannot be returned to by path.
    pub path: Option<String>,
    pub line: usize,
    pub column: usize,
}

/// Places the cursor has jumped from, and where you are within that history.
///
/// A list with a cursor rather than a stack, because `ctrl-o` and `ctrl-i` walk *both* ways: a stack
/// can only go back, and going back without being able to return is a trap rather than a feature.
#[derive(Debug, Clone, Default)]
pub struct JumpList {
    entries: Vec<Jump>,
    /// Where in `entries` the cursor is. Equal to `entries.len()` when at the present.
    at: usize,
}

impl JumpList {
    /// How many jumps are remembered. Vim keeps 100; there is no reason to differ.
    pub const CAPACITY: usize = 100;

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Records a position being jumped away from.
    ///
    /// Jumping after having walked back discards the forward history, exactly as a new edit discards
    /// the redo stack: keeping it would let `ctrl-i` return to a future that no longer follows from
    /// where you are.
    pub fn push(&mut self, jump: Jump) {
        self.entries.truncate(self.at);

        // Consecutive jumps to the same line are one entry. Otherwise `n` through a file's matches
        // fills the list with a hundred entries on the same screen.
        if self
            .entries
            .last()
            .is_some_and(|last| last.path == jump.path && last.line.abs_diff(jump.line) < 2)
        {
            self.entries.pop();
        }

        self.entries.push(jump);
        if self.entries.len() > Self::CAPACITY {
            self.entries.remove(0);
        }
        self.at = self.entries.len();
    }

    /// Steps back, returning where to go. `here` is the current position, remembered so that
    /// `ctrl-i` can come back to it.
    pub fn back(&mut self, here: Jump) -> Option<Jump> {
        if self.at == 0 {
            return None;
        }
        if self.at == self.entries.len() {
            // Stepping off the present for the first time: record it, so forward has a destination.
            self.entries.push(here);
        }
        self.at -= 1;
        self.entries.get(self.at).cloned()
    }

    pub fn forward(&mut self) -> Option<Jump> {
        if self.at + 1 >= self.entries.len() {
            return None;
        }
        self.at += 1;
        self.entries.get(self.at).cloned()
    }
}

/// Whether an action is a *jump* — one that moves far enough that you might want to come back.
///
/// Vim's list, and for Vim's reason: `j` and `w` are how you move around what you are reading, and
/// recording them would fill the jumplist with places you never left.
pub fn is_jump(action: &Action) -> bool {
    match action {
        Action::Move { motion, .. } | Action::Extend { motion, .. } => matches!(
            motion,
            Motion::FileStart
                | Motion::FileEnd
                | Motion::ToLine(_)
                | Motion::ParagraphForward
                | Motion::ParagraphBackward
                | Motion::MatchingBracket
        ),
        Action::GotoDefinition
        | Action::SearchProject
        | Action::RepeatSearch { .. }
        | Action::NextDiagnostic { .. } => true,
        _ => false,
    }
}

/// Recording and replaying a sequence of actions.
///
/// Recorded as Actions rather than as keystrokes. Keystrokes would replay the *grammar* as well as
/// the command, so a macro recorded with one keymap would do something else under another — and a
/// macro that breaks when you rebind a key is a macro you cannot trust.
#[derive(Debug, Clone, Default)]
pub struct Macros {
    /// Which register is being recorded into, and what has been recorded so far.
    recording: Option<(char, Vec<Action>)>,
    /// Finished recordings, by register.
    stored: Vec<(char, Vec<Action>)>,
    /// The last register replayed, for `@@`.
    last: Option<char>,
}

impl Macros {
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// The register being recorded into, for the statusline. An editor that is silently recording is
    /// an editor that surprises you later.
    pub fn recording_register(&self) -> Option<char> {
        self.recording.as_ref().map(|(register, _)| *register)
    }

    /// Starts recording, or stops if already recording. Returns true if recording stopped.
    pub fn toggle(&mut self, register: char) -> bool {
        if let Some((register, actions)) = self.recording.take() {
            // Nothing recorded — an accidental `qq` — leaves whatever was there alone. Clearing it
            // first and then declining to store would silently empty a register being relied on.
            if !actions.is_empty() {
                self.stored.retain(|(existing, _)| *existing != register);
                self.stored.push((register, actions));
            }
            return true;
        }
        self.recording = Some((register, Vec::new()));
        false
    }

    /// Records one action, if recording.
    ///
    /// The recording controls themselves are never recorded: a macro that stops recording when
    /// replayed would end the recording you are making with it.
    pub fn record(&mut self, action: &Action) {
        if matches!(
            action,
            Action::RecordMacro { .. } | Action::ReplayMacro { .. }
        ) {
            return;
        }
        if let Some((_, actions)) = &mut self.recording {
            actions.push(action.clone());
        }
    }

    /// The actions stored in a register. `@@` replays the last one used.
    pub fn replay(&mut self, register: Option<char>) -> Option<Vec<Action>> {
        let register = register.or(self.last)?;
        self.last = Some(register);
        self.stored
            .iter()
            .find(|(stored, _)| *stored == register)
            .map(|(_, actions)| actions.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jump(line: usize) -> Jump {
        Jump {
            path: Some("a.rs".into()),
            line,
            column: 0,
        }
    }

    #[test]
    fn walking_back_and_forward_returns_to_where_you_were() {
        let mut list = JumpList::default();
        list.push(jump(10));
        list.push(jump(50));

        assert_eq!(list.back(jump(90)), Some(jump(50)));
        assert_eq!(list.back(jump(50)), Some(jump(10)));
        assert_eq!(
            list.forward(),
            Some(jump(50)),
            "forward retraces the same path"
        );
        assert_eq!(list.forward(), Some(jump(90)), "back to where it started");
        assert_eq!(list.forward(), None, "and no further");
    }

    #[test]
    fn a_new_jump_after_going_back_discards_the_forward_history() {
        // The same rule as undo and redo: keeping it would let forward return to a future that no
        // longer follows from where you are.
        let mut list = JumpList::default();
        list.push(jump(10));
        list.push(jump(20));
        list.back(jump(30));

        list.push(jump(70));
        assert_eq!(list.forward(), None);
    }

    #[test]
    fn consecutive_jumps_to_the_same_place_are_one_entry() {
        // `n` through matches on one screen should not fill the list with a hundred entries.
        let mut list = JumpList::default();
        list.push(jump(40));
        list.push(jump(40));
        list.push(jump(41));

        assert_eq!(list.len(), 1);
    }

    #[test]
    fn the_list_is_bounded() {
        let mut list = JumpList::default();
        for line in 0..500 {
            list.push(jump(line * 10));
        }
        assert_eq!(list.len(), JumpList::CAPACITY);
    }

    #[test]
    fn going_back_from_an_empty_list_does_nothing() {
        let mut list = JumpList::default();
        assert_eq!(list.back(jump(1)), None);
        assert_eq!(list.forward(), None);
    }

    #[test]
    fn only_far_reaching_motions_count_as_jumps() {
        assert!(is_jump(&Action::Move {
            motion: Motion::FileEnd,
            count: 1
        }));
        assert!(is_jump(&Action::GotoDefinition));
        assert!(is_jump(&Action::RepeatSearch { forward: true }));

        // The motions you use to read with are not jumps, or the list fills with places you never
        // left.
        assert!(!is_jump(&Action::Move {
            motion: Motion::Down,
            count: 1
        }));
        assert!(!is_jump(&Action::Move {
            motion: Motion::WordForward,
            count: 1
        }));
        assert!(!is_jump(&Action::Undo));
    }

    #[test]
    fn a_macro_records_and_replays_actions() {
        let mut macros = Macros::default();
        assert!(!macros.toggle('q'), "the first press starts recording");
        assert!(macros.is_recording());
        assert_eq!(macros.recording_register(), Some('q'));

        macros.record(&Action::DeleteForward);
        macros.record(&Action::Newline);

        assert!(macros.toggle('q'), "the second press stops it");
        assert!(!macros.is_recording());

        assert_eq!(
            macros.replay(Some('q')),
            Some(vec![Action::DeleteForward, Action::Newline])
        );
    }

    #[test]
    fn the_recording_controls_are_not_themselves_recorded() {
        // Otherwise replaying a macro would stop the recording you are making with it.
        let mut macros = Macros::default();
        macros.toggle('q');
        macros.record(&Action::RecordMacro { register: 'q' });
        macros.record(&Action::ReplayMacro {
            register: Some('w'),
        });
        macros.record(&Action::Undo);
        macros.toggle('q');

        assert_eq!(macros.replay(Some('q')), Some(vec![Action::Undo]));
    }

    #[test]
    fn replaying_without_a_register_repeats_the_last_one() {
        let mut macros = Macros::default();
        macros.toggle('a');
        macros.record(&Action::Undo);
        macros.toggle('a');

        assert!(macros.replay(Some('a')).is_some());
        assert_eq!(
            macros.replay(None),
            Some(vec![Action::Undo]),
            "`@@` repeats the last register used"
        );
    }

    #[test]
    fn recording_over_a_register_replaces_it() {
        let mut macros = Macros::default();
        macros.toggle('q');
        macros.record(&Action::Undo);
        macros.toggle('q');

        macros.toggle('q');
        macros.record(&Action::Redo);
        macros.toggle('q');

        assert_eq!(macros.replay(Some('q')), Some(vec![Action::Redo]));
    }

    #[test]
    fn an_empty_recording_is_not_stored() {
        // Pressing `qq` twice by accident should not silently empty a register you rely on.
        let mut macros = Macros::default();
        macros.toggle('q');
        macros.record(&Action::Undo);
        macros.toggle('q');

        macros.toggle('q');
        macros.toggle('q');

        assert_eq!(
            macros.replay(Some('q')),
            Some(vec![Action::Undo]),
            "the previous recording survives"
        );
    }

    #[test]
    fn replaying_an_empty_register_yields_nothing() {
        let mut macros = Macros::default();
        assert_eq!(macros.replay(Some('z')), None);
        assert_eq!(macros.replay(None), None);
    }
}

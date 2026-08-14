//! The system clipboard, bridged to the unnamed register.
//!
//! There was none. `ctrl-c` filled a register inside the editor and `ctrl-v` read it back, so copying
//! in vex and pasting in a browser produced nothing at all — the single clunkiest thing about the
//! non-modal mode, and not something a shortcut can fix. Vim users have the same expectation under a
//! different name: `set clipboard=unnamedplus` is the first line of most people's config.
//!
//! **The unnamed register is the clipboard**, in both directions:
//!
//! - anything that yanks or deletes into the unnamed register pushes it out, and
//! - a paste pulls the system clipboard in first, so text copied in another application is there.
//!
//! Named registers (`"ayy`) stay private to the editor. That is the Vim contract and it is also the
//! useful one: registers are a scratchpad, and a scratchpad that fights with every other application
//! for the clipboard is not one.
//!
//! Failure is silent and total: no clipboard on the system (a bare tty, a locked-down container, a
//! Wayland session without a data device) means the internal register still works exactly as before.
//! An editor that refused to copy because it could not reach a clipboard daemon would be worse than
//! one that quietly kept its own.

use std::sync::Mutex;

/// The handle, opened once and kept.
///
/// Opening a clipboard connection costs a round trip to the display server, and copy is not a rare
/// keystroke. `OnceLock` will not do: `arboard::Clipboard` needs `&mut self` for both operations.
static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

/// Runs `f` against the system clipboard, or does nothing if there is not one.
fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Option<T> {
    // A poisoned lock means another thread panicked mid-copy. The clipboard is not worth propagating
    // a panic over, so the poison is stepped over rather than unwrapped.
    let mut guard = match CLIPBOARD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if guard.is_none() {
        *guard = arboard::Clipboard::new().ok();
    }
    f(guard.as_mut()?).ok()
}

/// Puts text on the system clipboard.
pub fn set(text: &str) {
    // Empty is not written: clearing the clipboard because a yank matched nothing would throw away
    // whatever the user copied somewhere else.
    if text.is_empty() {
        return;
    }
    with(|clipboard| clipboard.set_text(text.to_string()));
}

/// What is on the system clipboard, if anything readable is.
pub fn get() -> Option<String> {
    with(|clipboard| clipboard.get_text()).filter(|text| !text.is_empty())
}

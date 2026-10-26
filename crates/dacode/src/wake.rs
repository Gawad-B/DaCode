//! How background workers tell the event loop they have an answer.
//!
//! The loop sleeps when there is no input. A worker that finishes while it sleeps must wake it, or
//! its answer waits for the next keypress — which is exactly how syntax colours came to appear only
//! after a click.

/// Called from a worker thread when a result is ready. Cheap and non-blocking.
///
/// Keep in sync: `dacode-syntax`, `dacode-project` and `dacode-lsp` sit below this crate and each
/// keep a private copy of this alias, which must stay identical to it.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// The user event the waker sends.
#[derive(Debug, Clone, Copy)]
pub struct Wake;

/// A waker that sends at most one event until [`coalescing`]'s flag is cleared.
///
/// A language server indexing prints a stderr line and queues an event many times a second; one
/// user event each would wake the loop hundreds of times a second to collect what a single wake
/// collects. The closure sends only when it flips `pending` from false to true, and the loop clears
/// the flag *before* it collects, so a result that lands mid-collection raises a fresh event
/// instead of being swallowed by a stale flag.
///
/// `send` is a parameter so this can be tested without an event loop.
pub fn coalescing(
    send: impl Fn() + Send + Sync + 'static,
) -> (Waker, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let pending = std::sync::Arc::new(AtomicBool::new(false));
    let flag = pending.clone();
    let waker: Waker = std::sync::Arc::new(move || {
        if !flag.swap(true, Ordering::AcqRel) {
            send();
        }
    });
    (waker, pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_burst_of_wakes_is_one_event_until_the_loop_has_looked() {
        // Indexing makes a language server chatty. The loop must be woken once for the burst, and
        // again for whatever arrives after it has collected — never lost, never one per message.
        let sent = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = sent.clone();
        let (waker, pending) = coalescing(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        waker();
        waker();
        waker();
        assert_eq!(sent.load(Ordering::SeqCst), 1, "three wakes, one event");

        pending.store(false, Ordering::SeqCst);
        waker();
        assert_eq!(
            sent.load(Ordering::SeqCst),
            2,
            "a wake after the loop cleared the flag must send again, or its result is stranded"
        );
    }
}

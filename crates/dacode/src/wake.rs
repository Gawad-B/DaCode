//! How background workers tell the event loop they have an answer.
//!
//! The loop sleeps when there is no input. A worker that finishes while it sleeps must wake it, or
//! its answer waits for the next keypress — which is exactly how syntax colours came to appear only
//! after a click.

/// Called from a worker thread when a result is ready. Cheap and non-blocking.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// The user event the waker sends.
#[derive(Debug, Clone, Copy)]
pub struct Wake;

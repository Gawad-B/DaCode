//! `vex` — the editor's top layer: chrome (layout and painting) and action execution.
//!
//! Exposed as a library so the editor can be driven headlessly in tests: key sequences in,
//! document text out, with no window and no GPU. That is the payoff of the `Action` design —
//! the same path the keyboard drives is the path the tests drive.

pub mod chrome;
pub mod completion;
pub mod config;
pub mod execute;
pub mod import_cli;
pub mod palette;
pub mod prompt;
pub mod recovery;
pub mod results;

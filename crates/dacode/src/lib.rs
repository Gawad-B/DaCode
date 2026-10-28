//! `dacode` — the editor's top layer: chrome (layout and painting) and action execution.
//!
//! Exposed as a library so the editor can be driven headlessly in tests: key sequences in,
//! document text out, with no window and no GPU. That is the payoff of the `Action` design —
//! the same path the keyboard drives is the path the tests drive.

pub mod blame;
pub mod chrome;
pub mod clipboard;
pub mod completion;
pub mod config;
pub mod edits;
pub mod execute;
pub mod focus;
pub mod format;
pub mod git;
pub mod help;
pub mod icon;
pub mod import_cli;
pub mod install;
pub mod layout;
pub mod menu;
pub mod motion;
pub mod open;
pub mod palette;
pub mod panes;
pub mod paths;
pub mod picker;
pub mod problems;
pub mod process;
pub mod prompt;
pub mod recovery;
pub mod results;
pub mod save;
pub mod scm;
pub mod scroll;
pub mod search_panel;
pub mod session;
pub mod settings_ui;
pub mod share;
pub mod snippet;
pub mod source_control;
pub mod terminal;
pub mod tree;
pub mod wake;
pub mod welcome;

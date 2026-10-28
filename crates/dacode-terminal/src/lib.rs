//! The integrated terminal: a shell on a pseudoterminal, its output parsed into a grid off the UI
//! thread, and the visible part of that grid copied out for drawing.
//!
//! The emulation itself — the grid, the scrollback, the VT parser and the PTY on both platforms — is
//! `alacritty_terminal`. A terminal emulator is a subsystem the size of several crates here, and the
//! way one fails when written in a hurry is a shell that eats your keystrokes or draws garbage after
//! the first full-screen program; alacritty's has had years of people running everything in it.
//! What this crate adds is the editor's side of the contract: keys spelled as a shell expects them
//! ([`keys`]), the mouse spelled as a full-screen program asked for it ([`mouse`]), the shell chosen per platform with the platform as a parameter ([`shell`]), a
//! viewport-sized [`Screen`] to draw from, and a [`Terminal`] that wakes the editor's event loop
//! the way every other background producer does.

pub mod keys;
pub mod mouse;
pub mod screen;
mod session;
pub mod shell;

pub use keys::{KeyModes, bytes_for, paste};
pub use mouse::{MouseAction, MouseButton, MouseEvent, MouseModes, MouseModifiers};
pub use screen::{Cell, CellPoint, Color, Cursor, Emulator, GridSize, Screen, SelectKind, Style};
pub use session::{Exit, Palette, SCROLLBACK, Spawn, Terminal, Waker};
pub use shell::{Platform, ShellCommand, shells_for};

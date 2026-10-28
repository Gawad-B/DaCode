//! A shell on a PTY, with its output parsed on a thread of its own.
//!
//! ## Threads
//!
//! alacritty's event loop owns the PTY: it reads, parses into the shared `Term`, and writes what
//! the UI sends it, all on a thread named "PTY reader". The UI thread does three things and each is
//! bounded: it **sends** bytes (a channel push), it **copies** the visible rows out under the lock
//! ([`Screen::capture`], viewport-sized), and it **asks** whether anything changed (an atomic). It
//! never reads from the PTY and never parses — a `cat` of a large file is the parser thread's
//! problem, and the editor keeps answering keys while it happens.
//!
//! The lock is alacritty's `FairMutex`, which the parser takes for at most 64 KiB of input at a
//! time before releasing it, so the UI's wait for it is bounded by one such batch.
//!
//! ## Waking
//!
//! The parser announces new output with `Event::Wakeup`, once per batch it reads — under a `yes`
//! flood, thousands of times a second. [`Terminal::take_dirty`] is the coalescing: the waker is
//! called only when the dirty flag goes from clear to set, and the flag is cleared only when the UI
//! actually copies the screen, which it does once per frame it draws. So a flood wakes the loop
//! once per frame and not once per read, and output that lands mid-frame raises a fresh wake rather
//! than being lost behind a stale flag.
//!
//! ## Spawning
//!
//! `dacode::process::command` is how every other child is started, because on Windows a console
//! child of a GUI application gets a console window of its own unless `CREATE_NO_WINDOW` is set.
//! The shell is the exception, and has to be: alacritty's `tty` spawns it itself — `openpty` and a
//! `setsid` child on Unix, `CreateProcessW` attached to a pseudoconsole on Windows. ConPTY is what
//! gives the child its console, so Windows has no reason to make it a visible one: the pseudoconsole
//! *is* its console, and nothing is shown. That is the same mechanism Windows Terminal and VSCode
//! use, and the reason neither shows a stray window either.

use std::borrow::Cow;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;

use alacritty_terminal::vte::ansi::{NamedColor, Rgb};

use crate::keys::{self, KeyModes};
use crate::mouse::{self, MouseEvent, MouseModes};
use crate::screen::{self, CellPoint, GridSize, Screen, SelectKind};
use crate::shell::{self, ShellCommand};

/// Called from the parser thread when there is something new to draw. Cheap and non-blocking.
///
/// This crate sits below the binary and cannot name `dacode::wake::Waker`, so it keeps its own copy
/// of the alias, as `dacode-syntax`, `dacode-project` and `dacode-lsp` do. It must stay identical.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// How many rows of history a terminal keeps. alacritty's default; the grid is allocated lazily, so
/// a terminal that never fills it never pays for it.
pub const SCROLLBACK: usize = 10_000;

/// What starting a terminal needs.
pub struct Spawn {
    /// Tried in order; the first that starts is the shell. See [`shell::shells_for`].
    pub shells: Vec<ShellCommand>,
    /// Where the shell starts: the project root, so `cargo build` or `npm test` work as typed.
    pub directory: PathBuf,
    pub size: GridSize,
    /// The pixel size of one cell, which a program can ask for (`CSI 14 t`) to size images.
    pub cell: (u16, u16),
    pub waker: Option<Waker>,
}

/// The colours the terminal is drawn in, for a program that asks what they are.
///
/// `OSC 10 ; ?` and `OSC 11 ; ?` ask for the foreground and background, and a program uses the
/// answer to decide between its light and dark colours — `vim`'s `background`, `delta`, `bat`. One
/// that gets no answer waits a moment and guesses, usually dark, which on a light theme is text the
/// colour of the panel. `OSC 4 ; n ; ?` asks for one of the sixteen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

impl Palette {
    /// The colour alacritty's index names: 0–15 the named colours, then its own numbers for the
    /// foreground and background.
    fn color(&self, index: usize) -> Option<[u8; 3]> {
        match index {
            0..=15 => Some(self.ansi[index]),
            i if i == NamedColor::Foreground as usize => Some(self.foreground),
            i if i == NamedColor::Background as usize => Some(self.background),
            _ => None,
        }
    }
}

/// How the shell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The exit code, when the platform reported one.
    pub code: Option<i32>,
}

/// State the parser thread writes and the UI reads.
struct Shared {
    dirty: AtomicBool,
    exited: Mutex<Option<Exit>>,
    title: Mutex<Option<String>>,
    waker: Option<Waker>,
    /// For the replies the terminal owes the program — a cursor-position report, the text area's
    /// size. Set once the event loop exists, which is after the listener that needs it.
    sender: OnceLock<EventLoopSender>,
    size: Mutex<WindowSize>,
    /// What a colour query is answered with; nothing until the editor says.
    palette: Mutex<Option<Palette>>,
}

impl Shared {
    fn mark_dirty(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            if let Some(waker) = &self.waker {
                waker();
            }
        }
    }

    fn reply(&self, text: String) {
        if let Some(sender) = self.sender.get() {
            let _ = sender.send(Msg::Input(Cow::Owned(text.into_bytes())));
        }
    }
}

#[derive(Clone)]
struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let shared = &self.0;
        match event {
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                shared.mark_dirty()
            }
            Event::PtyWrite(text) => shared.reply(text),
            Event::TextAreaSizeRequest(format) => {
                let size = *shared.size.lock().unwrap_or_else(|e| e.into_inner());
                shared.reply(format(size));
            }
            Event::Title(title) => {
                *shared.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(title);
                shared.mark_dirty();
            }
            Event::ResetTitle => {
                *shared.title.lock().unwrap_or_else(|e| e.into_inner()) = None;
                shared.mark_dirty();
            }
            Event::ChildExit(status) => {
                *shared.exited.lock().unwrap_or_else(|e| e.into_inner()) = Some(Exit {
                    code: status.code(),
                });
                shared.mark_dirty();
            }
            Event::Exit => {
                let mut exited = shared.exited.lock().unwrap_or_else(|e| e.into_inner());
                exited.get_or_insert(Exit { code: None });
                drop(exited);
                shared.mark_dirty();
            }
            Event::ColorRequest(index, format) => {
                let palette = *shared.palette.lock().unwrap_or_else(|e| e.into_inner());
                if let Some([r, g, b]) = palette.and_then(|palette| palette.color(index)) {
                    shared.reply(format(Rgb { r, g, b }));
                }
            }
            // Not answered: the clipboard (OSC 52) because a program writing to the system
            // clipboard unasked is a thing to opt into, not a default; the bell because there is
            // nowhere quiet to ring it.
            Event::ClipboardStore(..) | Event::ClipboardLoad(..) | Event::Bell => {}
        }
    }
}

/// A running shell.
pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    sender: EventLoopSender,
    thread: Option<JoinHandle<()>>,
    size: GridSize,
    cell: (u16, u16),
    /// Which of [`Spawn::shells`] started.
    shell: ShellCommand,
    /// Bytes handed to the shell. Lets a test prove a key went to the editor and not here.
    sent: AtomicU64,
}

fn window_size(size: GridSize, cell: (u16, u16)) -> WindowSize {
    WindowSize {
        num_lines: size.rows.min(u16::MAX as usize) as u16,
        num_cols: size.columns.min(u16::MAX as usize) as u16,
        cell_width: cell.0.max(1),
        cell_height: cell.1.max(1),
    }
}

impl Terminal {
    /// Starts the first of `spawn.shells` that will start.
    pub fn spawn(spawn: Spawn) -> io::Result<Self> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no shell to start");
        for shell in &spawn.shells {
            match Self::start(&spawn, shell) {
                Ok(terminal) => return Ok(terminal),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn start(spawn: &Spawn, shell: &ShellCommand) -> io::Result<Self> {
        let size = spawn.size;
        let window = window_size(size, spawn.cell);
        let shared = Arc::new(Shared {
            dirty: AtomicBool::new(false),
            exited: Mutex::new(None),
            title: Mutex::new(None),
            waker: spawn.waker.clone(),
            sender: OnceLock::new(),
            size: Mutex::new(window),
            palette: Mutex::new(None),
        });
        let listener = Listener(shared.clone());

        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));

        // `..Default::default()` is a no-op here and not on Windows, where `Options` has an
        // `escape_args` field this build cannot name.
        #[allow(clippy::needless_update)]
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell.program.clone(), shell.args.clone())),
            working_directory: Some(spawn.directory.clone()),
            // The last of the output is read before the exit is reported, so `sh -c 'echo hi'`
            // shows its `hi` rather than racing the exit to the screen.
            drain_on_exit: true,
            env: shell::environment().into_iter().collect(),
            ..Default::default()
        };
        let pty = tty::new(&options, window, 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        let _ = shared.sender.set(sender.clone());
        let handle = event_loop.spawn();
        // The loop returns itself and its state when it stops. Dropping them is what drops the
        // PTY — which hangs the shell up and reaps it — and that wait happens here, on the parser's
        // own thread, never on the UI's.
        let thread = std::thread::Builder::new()
            .name("PTY reaper".into())
            .spawn(move || {
                let _ = handle.join();
            })?;

        Ok(Self {
            term,
            shared,
            sender,
            thread: Some(thread),
            size,
            cell: spawn.cell,
            shell: shell.clone(),
            sent: AtomicU64::new(0),
        })
    }

    /// The shell that started.
    pub fn shell(&self) -> &ShellCommand {
        &self.shell
    }

    /// Sends bytes to the shell — keys, or a paste. A view scrolled back into history jumps to the
    /// bottom first, as it does in every terminal: typing is about the prompt, not the history.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if bytes.is_empty() || self.exit().is_some() {
            return;
        }
        {
            let mut term = self.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
                drop(term);
                self.shared.mark_dirty();
            }
        }
        self.sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let _ = self.sender.send(Msg::Input(bytes));
    }

    /// Sends one key, spelled for the mode the terminal is in. Returns whether it had a spelling.
    pub fn key(&self, key: dacode_input::Key) -> bool {
        match keys::bytes_for(key, self.key_modes()) {
            Some(bytes) => {
                self.write(bytes);
                true
            }
            None => false,
        }
    }

    /// Sends pasted text, bracketed if the shell asked for that.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        self.write(keys::paste(text, bracketed));
    }

    fn key_modes(&self) -> KeyModes {
        KeyModes {
            app_cursor: self.term.lock().mode().contains(TermMode::APP_CURSOR),
        }
    }

    /// How many bytes have been sent to the shell since it started.
    pub fn bytes_sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Scrolls the view by `rows`: positive towards older output, as a wheel turned up does.
    ///
    /// A full-screen program (`less`, `man`, `vim`) runs on the alternate screen, which has no
    /// history to scroll; there the wheel is sent as arrow keys instead, which is xterm's
    /// "alternate scroll" and what makes a wheel scroll a pager.
    pub fn scroll(&self, rows: i32) {
        if rows == 0 {
            return;
        }
        let modes = self.mouse_modes();
        if modes.alt_screen && modes.alternate_scroll && !modes.reporting() {
            self.write(mouse::alternate_scroll(rows, modes.app_cursor));
            return;
        }
        self.term.lock().scroll_display(Scroll::Delta(rows));
        self.shared.mark_dirty();
    }

    /// What the program asked the mouse to do.
    pub fn mouse_modes(&self) -> MouseModes {
        MouseModes::from_mode(*self.term.lock().mode())
    }

    /// Reports a mouse event to the program, if it asked for that kind. Returns whether it did.
    ///
    /// Written straight to the PTY rather than through [`Terminal::write`]: a program with the
    /// mouse is on the alternate screen, which has no history to jump back from, and a release
    /// arriving after the shell exited is simply not sent.
    pub fn mouse(&self, event: MouseEvent) -> bool {
        let Some(bytes) = mouse::encode(&event, &self.mouse_modes()) else {
            return false;
        };
        if self.exit().is_some() {
            return false;
        }
        self.sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let _ = self.sender.send(Msg::Input(Cow::Owned(bytes)));
        true
    }

    /// Starts a mouse selection at a visible cell, replacing any other.
    pub fn select(&self, kind: SelectKind, at: CellPoint) {
        screen::select(&mut self.term.lock(), kind, at);
        self.shared.mark_dirty();
    }

    /// Moves the selection's far end to a visible cell, as a drag does.
    pub fn extend_selection(&self, at: CellPoint) {
        screen::extend_selection(&mut self.term.lock(), at);
        self.shared.mark_dirty();
    }

    /// Whether anything is selected.
    pub fn has_selection(&self) -> bool {
        self.term.lock().selection.is_some()
    }

    /// Drops the selection.
    pub fn clear_selection(&self) {
        let mut term = self.term.lock();
        if term.selection.take().is_some() {
            drop(term);
            self.shared.mark_dirty();
        }
    }

    /// The selected text, as copying puts it on the clipboard. `None` with nothing selected.
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// The colours to answer a program's colour query with. Cheap; asked whenever the theme may
    /// have changed.
    pub fn set_palette(&self, palette: Palette) {
        *self
            .shared
            .palette
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(palette);
    }

    /// Changes the grid to `size`, telling the shell so it can re-wrap its prompt. Nothing happens
    /// when the size is what it already was — this is asked every frame.
    pub fn resize(&mut self, size: GridSize, cell: (u16, u16)) {
        if size == self.size && cell == self.cell {
            return;
        }
        self.size = size;
        self.cell = cell;
        let window = window_size(size, cell);
        *self.shared.size.lock().unwrap_or_else(|e| e.into_inner()) = window;
        self.term.lock().resize(size);
        let _ = self.sender.send(Msg::Resize(window));
        self.shared.mark_dirty();
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    /// Whether there is output the screen does not show yet. Leaves the flag set.
    pub fn is_dirty(&self) -> bool {
        self.shared.dirty.load(Ordering::Acquire)
    }

    /// Clears the dirty flag and copies the screen. Cleared *before* copying, so output that lands
    /// while the copy is made raises the flag — and the waker — again rather than being lost.
    pub fn take_screen(&self) -> Screen {
        self.shared.dirty.store(false, Ordering::Release);
        self.screen()
    }

    /// The visible rows, without touching the dirty flag.
    pub fn screen(&self) -> Screen {
        Screen::capture(&self.term.lock())
    }

    /// The title the shell set, if it set one.
    pub fn title(&self) -> Option<String> {
        self.shared
            .title
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// How the shell ended, once it has.
    pub fn exit(&self) -> Option<Exit> {
        *self.shared.exited.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ends the shell and waits up to `within` for its thread to finish. Returns whether it did.
    ///
    /// Dropping a terminal ends it too, without waiting; this is for tests, which must not leave a
    /// shell behind them and must be able to prove they did not.
    pub fn shutdown(mut self, within: Duration) -> bool {
        let _ = self.sender.send(Msg::Shutdown);
        let Some(thread) = self.thread.take() else {
            return true;
        };
        let deadline = Instant::now() + within;
        while !thread.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = thread.join();
        true
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The event loop drops the PTY on its way out, which sends the shell SIGHUP (or closes the
        // pseudoconsole) and reaps it — on its own thread, so closing a terminal never waits here.
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A program the platform guarantees, running `script`: `sh -c` on Unix, `cmd /c` on Windows.
    /// Not `$SHELL` — a test that ran the developer's own shell would be testing their zshrc.
    fn run(script: &str) -> Vec<ShellCommand> {
        if cfg!(windows) {
            vec![ShellCommand::new("cmd", &["/c", script])]
        } else {
            vec![ShellCommand::new("sh", &["-c", script])]
        }
    }

    fn spawn(shells: Vec<ShellCommand>, waker: Option<Waker>) -> Terminal {
        Terminal::spawn(Spawn {
            shells,
            directory: std::env::temp_dir(),
            size: GridSize::new(40, 10),
            cell: (8, 16),
            waker,
        })
        .expect("a platform shell starts")
    }

    fn wait_for(terminal: &Terminal, within: Duration, done: impl Fn(&Screen) -> bool) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if done(&terminal.screen()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_shell_runs_and_its_output_reaches_the_grid() {
        // The whole subsystem end to end: a real PTY, a real child, the parser on its own thread
        // and the screen copied out on this one.
        let terminal = spawn(run("echo hello"), None);
        let seen = wait_for(&terminal, Duration::from_secs(10), |screen| {
            screen.lines().iter().any(|line| line.trim() == "hello")
        });
        let lines = terminal.screen().lines();
        assert!(
            terminal.shutdown(Duration::from_secs(10)),
            "the shell's thread ended"
        );
        assert!(seen, "hello on screen: {lines:?}");
    }

    #[test]
    fn the_shell_ending_is_reported() {
        let terminal = spawn(run("exit 3"), None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while terminal.exit().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let exit = terminal.exit();
        assert!(terminal.shutdown(Duration::from_secs(10)));
        let exit = exit.expect("the exit is seen");
        if !cfg!(windows) {
            assert_eq!(exit.code, Some(3));
        }
    }

    #[test]
    fn typed_keys_reach_the_shell() {
        // An interactive `sh` (or `cmd`), driven by keys the way the panel drives it.
        let shells = if cfg!(windows) {
            vec![ShellCommand::new("cmd", &[])]
        } else {
            vec![ShellCommand::new("sh", &[])]
        };
        let terminal = spawn(shells, None);
        for key in dacode_input::Key::parse_sequence("echo typed$((40+2))<cr>") {
            terminal.key(key);
        }
        if cfg!(windows) {
            for key in dacode_input::Key::parse_sequence("<cr>") {
                terminal.key(key);
            }
        }
        let seen = wait_for(&terminal, Duration::from_secs(10), |screen| {
            screen
                .lines()
                .iter()
                .any(|line| line.trim() == "typed42" || line.trim() == "typed$((40+2))")
        });
        let lines = terminal.screen().lines();
        assert!(terminal.bytes_sent() > 0);
        assert!(terminal.shutdown(Duration::from_secs(10)), "sh was ended");
        assert!(seen, "the command ran: {lines:?}");
    }

    #[test]
    fn a_flood_of_output_wakes_the_loop_once_until_it_looks() {
        // `yes` would wake the loop thousands of times a second if every batch the parser read
        // raised an event. The flag is cleared only when the screen is copied, so however much
        // arrives before that, the loop is woken once.
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let waker: Waker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let script = if cfg!(windows) {
            "for /L %i in (1,1,3000) do @echo line %i"
        } else {
            "i=0; while [ $i -lt 3000 ]; do echo line $i; i=$((i+1)); done"
        };
        let terminal = spawn(run(script), Some(waker));
        let deadline = Instant::now() + Duration::from_secs(20);
        while terminal.exit().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "thousands of lines, one wake, because nobody copied the screen"
        );

        // Once the screen is taken, the next output wakes again — never lost.
        let _ = terminal.take_screen();
        terminal.scroll(1);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert!(terminal.shutdown(Duration::from_secs(10)));
    }

    #[test]
    fn a_click_reaches_a_program_only_once_it_asks_for_the_mouse() {
        // A shell at a prompt has not asked: a click there is a selection, and `ESC [ <` typed at
        // the prompt would be garbage. The same click after the program turns reporting on is its.
        if cfg!(windows) {
            return;
        }
        let terminal = spawn(
            run("sleep 1; printf '\\033[?1000h\\033[?1006h'; sleep 10"),
            None,
        );
        let click = MouseEvent {
            button: crate::MouseButton::Left,
            action: crate::MouseAction::Press,
            row: 0,
            column: 0,
            modifiers: Default::default(),
        };
        assert!(!terminal.mouse(click), "not before it asked");
        let before = terminal.bytes_sent();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !terminal.mouse_modes().reporting() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let modes = terminal.mouse_modes();
        let sent = terminal.mouse(click);
        let after = terminal.bytes_sent();
        assert!(terminal.shutdown(Duration::from_secs(10)));
        assert!(modes.click && modes.sgr, "{modes:?}");
        assert!(sent);
        assert_eq!(after - before, b"\x1b[<0;1;1M".len() as u64);
    }

    #[test]
    fn a_program_asking_the_background_colour_is_told_the_themes() {
        // `OSC 11 ; ?` is how a program picks its light or dark colours. The reply is written to
        // the program's input; `od` prints what it read, so the answer shows up on the screen.
        if cfg!(windows) {
            return;
        }
        let terminal = spawn(
            run(
                "stty -echo -icanon; sleep 1; printf '\\033]11;?\\007'; head -c 13 | od -An -c; sleep 5",
            ),
            None,
        );
        terminal.set_palette(Palette {
            foreground: [0xee, 0xee, 0xee],
            background: [0x12, 0x34, 0x56],
            ansi: [[0, 0, 0]; 16],
        });
        // `od` pads every byte to four columns and the grid wraps at forty, so the rows are read
        // joined: the reply is `rgb:1212/…`, the background's red twice.
        let answered = wait_for(&terminal, Duration::from_secs(10), |screen| {
            screen.lines().concat().contains("b   :   1   2   1   2")
        });
        let lines = terminal.screen().lines();
        assert!(terminal.shutdown(Duration::from_secs(10)));
        assert!(answered, "the reply was read back: {lines:?}");
    }

    #[test]
    fn a_resize_reaches_the_grid() {
        let mut terminal = spawn(run("sleep 5"), None);
        terminal.resize(GridSize::new(60, 7), (8, 16));
        let screen = terminal.screen();
        assert_eq!(screen.size, GridSize::new(60, 7));
        assert_eq!(screen.rows.len(), 7);
        assert!(
            terminal.shutdown(Duration::from_secs(10)),
            "sleep was ended"
        );
    }
}

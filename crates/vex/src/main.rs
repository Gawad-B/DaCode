//! `vex` — the editor binary.
//!
//! Wires the layers together: `vex-text` (storage), `vex-buffer` (document and undo),
//! `vex-display` (soft wrap and viewport), `vex-theme` (appearance), `vex-render` (GPU).
//!
//! The key handling here is a **stand-in** for `crates/input`. It switches modes and edits, but
//! it does not implement the real grammar: no counts, no registers, no operator-motion
//! composition, no dot-repeat. Those need the `Action` vocabulary and a proper state machine,
//! which is M3.

use anyhow::{Context, Result};
use std::sync::Arc;
use vex::chrome::{self, EditorState};
use vex::config;
use vex::execute::{Outcome, execute};
use vex::palette::{Palette, PaletteKind, Target};
use vex::prompt::{Command, PromptKind, parse_command};
use vex::recovery::Recovery;
use vex_buffer::Document;
use vex_display::{DisplayMap, ScrollAnchor, WrapMode};
use vex_input::{Action, Grammar, Key, KeyCode, Registers, Resolution};
use vex_project::{FileIndex, FileWalk, find_root};
use vex_render::{Frame, Rect, Renderer};
use vex_syntax::{Language, SyntaxRequest, SyntaxWorker};
use vex_text::{Selection, SelectionSet, Text, Version};
use vex_theme::{Mode, Theme};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// Rows of context kept between the cursor and the edge of the viewport.
const SCROLL_MARGIN: usize = 3;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `vex import …` is a separate operation: it rewrites configuration, so it happens when asked
    // rather than at startup.
    if args.first().map(String::as_str) == Some("import") {
        return vex::import_cli::run(&args);
    }

    // `--screenshot <png> [--mode <name>] [--light] [file]` renders one frame offscreen and
    // exits: verifies the design with no window server, which is how it gets checked in CI and
    // over SSH.
    if let Some(index) = args.iter().position(|arg| arg == "--screenshot") {
        let output = args
            .get(index + 1)
            .context("--screenshot needs an output path")?
            .clone();
        return screenshot(&args, &output);
    }

    let path = positional(&args);
    let (text, crumbs, file_path) = load(path)?;

    // The project root anchors file discovery: a marker search from the opened file, so opening a
    // stray file does not index a home directory.
    let root = find_root(&std::path::PathBuf::from(
        file_path.clone().unwrap_or_else(|| ".".to_string()),
    ));
    eprintln!("vex: project root {}", root.display());

    let event_loop = EventLoop::new().context("creating the event loop")?;
    // Wait, not Poll: an editor redraws in response to input, not continuously. Polling would
    // spin the GPU and the fan for a screen that is not changing.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App {
        window: None,
        renderer: None,
        frame: Frame::default(),
        modifiers: ModifiersState::empty(),
        grammar: Grammar::new(),
        registers: Registers::new(),
        last_change: None,
        syntax: None,
        requested_range: None,
        lsp: None,
        sent_version: None,
        recovery: Recovery::new(),
        wakes: 0,
        index: FileIndex::new(root.clone()),
        walk: Some(FileWalk::start(root.clone())),
        buffers: Vec::new(),
        state: new_state(text, crumbs, file_path, Mode::Normal, None, root.clone()),
    };

    event_loop.run_app(&mut app).context("running the app")?;
    Ok(())
}

fn new_state(
    text: Text,
    path: Vec<String>,
    file_path: Option<String>,
    mode: Mode,
    theme_override: Option<Theme>,
    project_root: std::path::PathBuf,
) -> EditorState {
    // Config decides the theme and metrics unless the caller overrode them (the screenshot path
    // does, to render a specific appearance on demand).
    let settings = config::load();
    if let Some(source) = &settings.theme_source {
        eprintln!("vex: theme from {source}");
    }

    let display = DisplayMap {
        tab_width: settings.tab_width,
        mode: if settings.soft_wrap {
            WrapMode::Word
        } else {
            WrapMode::None
        },
        ..Default::default()
    };

    EditorState {
        mode,
        theme: theme_override.unwrap_or(settings.theme),
        typography: settings.typography,
        document: Document::from_text(text),
        display,
        scroll: ScrollAnchor::default(),
        path,
        file_path,
        pending: String::new(),
        // No parse has landed yet. The ledger shows this as stale until the first result
        // arrives, which is exactly what it is.
        syntax_version: Version(0),
        diagnostics_version: Version(0),
        highlights: Vec::new(),
        node_path: Vec::new(),
        syntax_error: false,
        frame_ms: 0.0,
        worst_ms: 0.0,
        diagnostics: vex_lsp::Diagnostics::default(),
        server_status: "no language server".to_string(),
        palette: None,
        prompt: None,
        results: None,
        search_matches: Vec::new(),
        project_root,
        buffer_count: 1,
        active_buffer: 0,
    }
}

/// Flags that consume the argument after them. Without this list, a flag's *value* gets mistaken
/// for the file to open — which is how `--columns 60` ended up trying to open a file named `60`.
const VALUE_FLAGS: [&str; 6] = [
    "--screenshot",
    "--mode",
    "--columns",
    "--palette",
    "--search",
    "--grep",
];

/// The first non-flag argument: the file to open.
fn positional(args: &[String]) -> Option<String> {
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg.starts_with("--") {
            skip_next = VALUE_FLAGS.contains(&arg.as_str());
            continue;
        }
        return Some(arg.clone());
    }
    None
}

/// Breadcrumbs for a path, trimmed to the last few segments so the title bar stays readable.
fn relative_crumbs(path: &str) -> Vec<String> {
    let all: Vec<String> = path
        .trim_start_matches("./")
        .split('/')
        .map(str::to_string)
        .collect();
    if all.len() > 4 {
        all[all.len() - 4..].to_vec()
    } else {
        all
    }
}

/// Loads the file to display, defaulting to this project's own source.
fn load(path: Option<String>) -> Result<(Text, Vec<String>, Option<String>)> {
    let path = path.unwrap_or_else(|| "crates/vex-text/src/anchor.rs".to_string());
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let crumbs = path.split('/').map(str::to_string).collect();
            Ok((Text::from_str(&contents), crumbs, Some(path)))
        }
        // A missing file is a scratch buffer, not a fatal error — but it must not be silently
        // saveable over some other path, so `file_path` stays `None`.
        Err(err) => {
            eprintln!("vex: could not read {path}: {err} — opening a scratch buffer");
            Ok((
                Text::from_str("// vex — scratch buffer\n"),
                vec!["scratch".to_string()],
                None,
            ))
        }
    }
}

fn screenshot(args: &[String], output: &str) -> Result<()> {
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let mode = match value("--mode").as_deref() {
        Some("insert") => Mode::Insert,
        Some("visual") => Mode::Visual,
        Some("replace") => Mode::Replace,
        _ => Mode::Normal,
    };
    // The screenshot path renders a requested appearance, so it overrides config.
    let theme = if flag("--light") {
        Some(Theme::light())
    } else if flag("--dark") {
        Some(Theme::dark())
    } else {
        None
    };

    let (text, crumbs, file_path) = load(positional(args))?;

    let (width, height) = (1600u32, 1000u32);
    let root = find_root(&std::path::PathBuf::from("."));
    let mut state = new_state(text, crumbs, file_path, mode, theme, root);

    // Land somewhere with content so the cursor line, the coordinate panel and the cursor shape
    // are all exercised rather than sitting at 0:0.
    let cursor = state
        .document
        .text()
        .point_to_char(vex_text::Point::new(21, 12));
    state.document.set_primary_caret(cursor);
    if mode == Mode::Visual {
        state
            .document
            .set_selections(SelectionSet::single(Selection::new(
                cursor,
                vex_text::CharOffset(cursor.get() + 24),
            )));
    }
    state.pending = if mode == Mode::Normal {
        "3d".into()
    } else {
        String::new()
    };
    state.frame_ms = 3.1;
    state.worst_ms = 6.4;
    state.scroll = ScrollAnchor::at_line(8);
    fit_wrap_columns(&mut state, width as f32, 1.0);

    // `--columns N` forces a narrow wrap width, so soft wrap can be inspected without resizing
    // a window.
    if let Some(columns) = value("--columns").and_then(|v| v.parse::<usize>().ok()) {
        state.display.columns = columns.max(8);
    }
    if flag("--no-wrap") {
        state.display.mode = WrapMode::None;
    }

    // `--palette` renders the overlay with a real index of this project, so the design can be
    // inspected without driving the window.
    if let Some(query) = value("--palette") {
        let root = find_root(&std::path::PathBuf::from("."));
        let walk = FileWalk::start(root.clone());
        let mut index = FileIndex::new(root);

        // A one-shot render can afford to wait for the walk; the editor never does.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(result) = walk.poll() {
                index.set_files(result.files, result.truncated);
                break;
            }
            std::thread::yield_now();
        }

        let mut palette = Palette::new(PaletteKind::Files);
        palette.query = query;
        palette.refresh_files(&mut index);
        palette.move_selection(1);
        state.palette = Some(palette);
    }

    // `--search QUERY` shows the search prompt with live matches; `--grep QUERY` shows the results
    // panel. Both exist so the new UI is inspectable the same way the rest of the design was.
    if let Some(query) = value("--search") {
        let matches = vex_buffer::find_all(
            state.document.text(),
            &query,
            &vex_buffer::SearchOptions::default(),
        );
        let mut prompt =
            vex::prompt::Prompt::new(vex::prompt::PromptKind::Search { forward: true }, cursor);
        prompt.input = query;
        prompt.matches = matches.clone();
        state.search_matches = matches;
        state.prompt = Some(prompt);
    }
    if let Some(query) = value("--grep") {
        let mut results = vex::results::Results::start(&state.project_root, &query);
        // A one-shot render can wait for the search; the editor never does.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            results.poll();
            if results.finished {
                break;
            }
            std::thread::yield_now();
        }
        state.results = Some(results);
    }

    // The screenshot path parses synchronously: there is no event loop to poll a worker from, and
    // a one-shot render can afford to wait.
    if let Some(language) = state.file_path.as_deref().and_then(Language::from_path) {
        match vex_syntax::Highlighter::new(language) {
            Ok(mut highlighter) => {
                highlighter.parse(state.document.text());
                let text = state.document.text();
                state.highlights =
                    highlighter.highlights(text, vex_text::CharOffset::ZERO..text.len_chars());
                state.node_path = highlighter.node_path(text, cursor);
                state.syntax_error = highlighter.has_error();
                state.syntax_version = state.document.version();
            }
            Err(err) => eprintln!("vex: syntax unavailable: {err}"),
        }
    }

    let mut headless =
        vex_render::Headless::new(width, height).context("initializing offscreen rendering")?;
    let [serif, mono, sans] = headless.resolved_families();
    eprintln!("vex: fonts — serif {serif}, mono {mono}, sans {sans}");

    let mut frame = Frame::default();
    chrome::draw(
        &mut frame,
        &state,
        Rect::new(0.0, 0.0, width as f32, height as f32),
        1.0,
    );

    headless
        .capture_png(&frame, state.theme.bg, output)
        .with_context(|| format!("writing {output}"))?;
    eprintln!(
        "vex: wrote {output} — {} quads, {} text runs",
        frame.quads.len(),
        frame.texts.len()
    );
    Ok(())
}

/// Derives the wrap width from the code area's pixel width.
///
/// Soft wrap is in columns, so the window size has to be translated once per resize rather than
/// assumed. Getting this wrong is how text ends up wrapping in the wrong place after a resize.
fn fit_wrap_columns(state: &mut EditorState, window_width: f32, scale: f32) {
    let sidebar = (292.0 * scale).min(window_width * 0.32);
    let code_width = window_width - sidebar - 78.0 * scale - 24.0 * scale;
    let advance = state.typography.code_size * 0.6 * scale;
    state.display.columns = ((code_width / advance).floor() as usize).max(20);
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    frame: Frame,
    modifiers: ModifiersState,
    state: EditorState,
    /// The modal state machine. Keys go in, Actions come out.
    grammar: Grammar,
    registers: Registers,
    /// The last change, for `.`. Only changes are recorded, so `.` stays predictable.
    last_change: Option<Action>,
    /// The background highlighter. `None` for a language with no grammar, in which case the
    /// editor renders unhighlighted text rather than nothing.
    syntax: Option<SyntaxWorker>,
    /// The visible range the last syntax request asked about, so scrolling can re-request.
    requested_range: Option<std::ops::Range<vex_text::CharOffset>>,
    /// The project's files, for the palette.
    index: FileIndex,
    /// The walk that fills the index, while it is running.
    walk: Option<FileWalk>,
    /// The language server for the active buffer, when one could be started.
    lsp: Option<vex_lsp::LspClient>,
    /// The document version last sent to the server, so `didChange` is not sent redundantly.
    sent_version: Option<vex_text::Version>,
    /// Persists dirty buffers so a crash loses seconds, not work.
    recovery: Recovery,
    /// Counts event-loop wake-ups, for tracing only.
    wakes: u64,
    /// Buffers other than the active one. The active document lives in `state`, and switching swaps
    /// it with an entry here — one document is always the live one, which keeps the editing path
    /// free of "which buffer am I in?" checks.
    buffers: Vec<Buffer>,
}

/// A buffer that is not currently being edited.
struct Buffer {
    document: Document,
    path: Vec<String>,
    file_path: Option<String>,
    scroll: ScrollAnchor,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("vex")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 820.0));

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("vex: could not create a window: {err}");
                event_loop.exit();
                return;
            }
        };

        match Renderer::new(window.clone()) {
            Ok(renderer) => {
                let [serif, mono, sans] = renderer.resolved_families();
                // Report substitutions rather than silently rendering a different design.
                eprintln!("vex: fonts — serif {serif}, mono {mono}, sans {sans}");
                self.renderer = Some(renderer);
            }
            Err(err) => {
                eprintln!("vex: could not initialize the GPU: {err:#}");
                event_loop.exit();
                return;
            }
        }

        self.window = Some(window);

        // Start the highlighter for this file's language, if there is a grammar for it.
        if let Some(language) = self
            .state
            .file_path
            .as_deref()
            .and_then(Language::from_path)
        {
            match SyntaxWorker::spawn(language) {
                Ok(worker) => self.syntax = Some(worker),
                // Highlighting is a nicety; the editor must still open the file without it.
                Err(err) => eprintln!("vex: syntax highlighting unavailable: {err}"),
            }
        }
        self.restore_recovery();
        self.sync_syntax();
        self.start_language_server();
    }

    /// Runs after the event queue drains, before the loop sleeps.
    ///
    /// Background services are polled here — this is the only place that happens without user input.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if std::env::var_os("VEX_LSP_TRACE").is_some() {
            self.wakes += 1;
            if self.wakes % 20 == 0 {
                eprintln!(
                    "vex: {} wake-ups, services_busy={}",
                    self.wakes,
                    self.services_busy()
                );
            }
        }
        let mut changed = self.poll_walk();
        changed |= self.sync_lsp();
        if let Some(results) = &mut self.state.results {
            changed |= results.poll();
        }

        if changed {
            self.request_redraw();
        }

        // Sleep until input, or briefly if something is still working. 100ms is far below the
        // threshold where a diagnostic feels late, and a `try_recv` ten times a second costs
        // nothing measurable.
        event_loop.set_control_flow(if self.services_busy() {
            ControlFlow::WaitUntil(
                std::time::Instant::now() + std::time::Duration::from_millis(100),
            )
        } else {
            ControlFlow::Wait
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.persist_recovery(true);
                event_loop.exit();
            }

            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),

            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                let scale = self.scale();
                fit_wrap_columns(&mut self.state, size.width as f32, scale);
                self.sync_syntax();
                self.request_redraw();
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y as isize * 3,
                    MouseScrollDelta::PixelDelta(position) => -(position.y / 40.0) as isize,
                };
                self.scroll_by(lines);
                self.sync_syntax();
                self.request_redraw();
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                // Collect the file index *before* handling the key: `ctrl-p` must not open onto an
                // empty list because the walk finished a moment earlier and nobody looked.
                self.poll_walk();

                if self.handle_key(logical_key) {
                    // Quitting is the last chance to save unsaved work.
                    self.persist_recovery(true);
                    event_loop.exit();
                } else {
                    if self.state.palette.is_none() && self.state.prompt.is_none() {
                        self.reveal_cursor();
                        self.sync_syntax();
                        self.sync_lsp();
                    }
                    self.persist_recovery(false);
                    self.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => self.redraw(),

            _ => {}
        }
    }
}

impl App {
    fn scale(&self) -> f32 {
        self.window
            .as_ref()
            .map(|window| window.scale_factor() as f32)
            .unwrap_or(1.0)
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn code_rows(&self) -> usize {
        let Some(renderer) = &self.renderer else {
            return 1;
        };
        let scale = self.scale();
        let (_, height) = renderer.size();
        let row_height = self.state.typography.row_height() * scale;
        let code_height = self.state.code_height(height as f32, scale);
        self.state.visible_rows(code_height, row_height)
    }

    /// Scrolls the viewport so the cursor stays visible with a margin of context.
    fn reveal_cursor(&mut self) {
        let height = self.code_rows();
        let cursor = self.state.cursor();
        self.state.scroll = self.state.display.scroll_to_reveal(
            self.state.document.text(),
            self.state.scroll,
            height,
            cursor,
            SCROLL_MARGIN,
        );
    }

    fn scroll_by(&mut self, rows: isize) {
        let text = self.state.document.text().clone();
        let mut anchor = self.state.scroll;

        for _ in 0..rows.unsigned_abs() {
            if rows > 0 {
                let in_line = self.state.display.rows_in_line(&text, anchor.line);
                if anchor.sub_row + 1 < in_line {
                    anchor.sub_row += 1;
                } else if anchor.line < text.last_line() {
                    anchor.line += 1;
                    anchor.sub_row = 0;
                }
            } else if anchor.sub_row > 0 {
                anchor.sub_row -= 1;
            } else if anchor.line > 0 {
                anchor.line -= 1;
                anchor.sub_row = self
                    .state
                    .display
                    .rows_in_line(&text, anchor.line)
                    .saturating_sub(1);
            }
        }

        self.state.scroll = anchor;
    }

    /// Translates a platform key event into the editor's own key type. Keeping `winit` out of
    /// the grammar is what lets the grammar be driven from tests, macros and Lua config.
    fn translate(&self, key: &WinitKey) -> Option<Key> {
        let code = match key.as_ref() {
            WinitKey::Named(NamedKey::Escape) => KeyCode::Escape,
            WinitKey::Named(NamedKey::Enter) => KeyCode::Enter,
            WinitKey::Named(NamedKey::Tab) => KeyCode::Tab,
            WinitKey::Named(NamedKey::Backspace) => KeyCode::Backspace,
            WinitKey::Named(NamedKey::Delete) => KeyCode::Delete,
            WinitKey::Named(NamedKey::ArrowLeft) => KeyCode::Left,
            WinitKey::Named(NamedKey::ArrowRight) => KeyCode::Right,
            WinitKey::Named(NamedKey::ArrowUp) => KeyCode::Up,
            WinitKey::Named(NamedKey::ArrowDown) => KeyCode::Down,
            WinitKey::Named(NamedKey::Home) => KeyCode::Home,
            WinitKey::Named(NamedKey::End) => KeyCode::End,
            WinitKey::Named(NamedKey::PageUp) => KeyCode::PageUp,
            WinitKey::Named(NamedKey::PageDown) => KeyCode::PageDown,
            WinitKey::Named(NamedKey::Space) => KeyCode::Char(' '),
            WinitKey::Character(text) => KeyCode::Char(text.chars().next()?),
            _ => return None,
        };

        Some(Key {
            code,
            ctrl: self.modifiers.control_key(),
            alt: self.modifiers.alt_key(),
        })
    }

    /// Returns true if the editor should quit.
    fn handle_key(&mut self, key: WinitKey) -> bool {
        let Some(key) = self.translate(&key) else {
            return false;
        };

        // While the palette is open it owns the keyboard: typing `i` into a file query must search
        // for `i`, not enter insert mode. The design says mode keys are swallowed, and they are.
        if self.state.palette.is_some() {
            self.handle_palette_key(key);
            return false;
        }
        if self.state.prompt.is_some() {
            return self.handle_prompt_key(key);
        }
        if self.state.results.is_some() && self.handle_results_key(key) {
            return false;
        }

        // Opening the palette is a Ctrl binding, so it works from any mode.
        if key.ctrl {
            match key.code {
                KeyCode::Char('p') => {
                    self.open_palette(PaletteKind::Files);
                    return false;
                }
                KeyCode::Char('b') => {
                    self.open_palette(PaletteKind::Buffers);
                    return false;
                }
                KeyCode::Char('f') => {
                    // Project search for the word under the cursor, or a `:grep` prompt when there
                    // is no word there.
                    let rows = self.code_rows();
                    execute(
                        &mut self.state,
                        &mut self.registers,
                        &Action::SearchProject,
                        rows,
                    );
                    return false;
                }
                _ => {}
            }
        }

        let resolution = self.grammar.feed(self.state.mode, key);
        match resolution {
            Resolution::Action(action) => self.run(action),
            Resolution::Pending(pending) => {
                // Show the partially-typed command in the statusline, so the machine's state is
                // visible rather than guessed at.
                self.state.pending = pending;
                false
            }
            Resolution::Unhandled => {
                self.state.pending.clear();
                false
            }
        }
    }

    fn run(&mut self, action: Action) -> bool {
        self.state.pending = self.grammar.pending().to_string();

        // `.` replays the last change rather than re-deriving it, which is the whole reason
        // actions are values.
        let action = if action == Action::RepeatChange {
            match self.last_change.clone() {
                Some(previous) => previous,
                None => return false,
            }
        } else {
            action
        };

        if action.is_change() {
            self.last_change = Some(action.clone());
        }

        let rows = self.code_rows();
        let outcome = execute(&mut self.state, &mut self.registers, &action, rows);

        // Font size changes the column advance, so the wrap width has to be recomputed or text
        // keeps wrapping at the old width.
        if matches!(action, Action::IncreaseFontSize | Action::DecreaseFontSize) {
            let (width, scale) = (self.window_width(), self.scale());
            fit_wrap_columns(&mut self.state, width, scale);
        }

        matches!(outcome, Outcome::Quit)
    }

    fn window_width(&self) -> f32 {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.size().0 as f32)
            .unwrap_or(1280.0)
    }

    /// The char range currently on screen, which bounds every syntax request.
    fn visible_range(&self) -> std::ops::Range<vex_text::CharOffset> {
        let text = self.state.document.text();
        let rows = self
            .state
            .display
            .rows_from(text, self.state.scroll, self.code_rows());

        let start = rows
            .first()
            .map(|row| row.range.start)
            .unwrap_or(vex_text::CharOffset::ZERO);
        let end = rows
            .last()
            .map(|row| row.range.end)
            .unwrap_or_else(|| text.len_chars());
        start..end
    }

    /// Hands new work to the highlighter and applies any result that is still fresh.
    ///
    /// This is the whole 60fps contract in one function: the UI thread only ever *sends*
    /// snapshots and *checks* versions. It never waits, and it never applies a result whose
    /// version has been superseded.
    /// Starts a language server for the active buffer, if one is known and installed.
    ///
    /// Failure is reported in the statusline rather than being fatal or silent: "no diagnostics" and
    /// "no server" look identical to a user, so they must not read the same.
    fn start_language_server(&mut self) {
        self.lsp = None;
        self.sent_version = None;
        self.state.diagnostics.clear();

        let Some(path) = self.state.file_path.clone() else {
            self.state.server_status = "scratch buffer".into();
            return;
        };
        let Some(spec) = vex_lsp::server_for(&path) else {
            self.state.server_status = "no server for this language".into();
            return;
        };
        if !vex_lsp::is_installed(spec.program) {
            self.state.server_status = format!("{} not installed", spec.program);
            return;
        }

        let root = self.index.root().to_path_buf();
        match vex_lsp::LspClient::start(spec.program, spec.args, &root) {
            Ok(client) => {
                eprintln!("vex: starting {} in {}", spec.program, root.display());
                self.state.server_status = format!("{} starting", spec.program);
                self.lsp = Some(client);
            }
            Err(error) => {
                eprintln!("vex: {error:#}");
                self.state.server_status = format!("{} failed to start", spec.program);
            }
        }
    }

    /// Sends document changes and applies whatever the server has said.
    ///
    /// The same contract as the syntax worker: send, poll, and refuse anything computed against a
    /// superseded version.
    fn sync_lsp(&mut self) -> bool {
        let Some(client) = &mut self.lsp else {
            return false;
        };
        let mut changed = false;
        let buffer_version = self.state.document.version();

        // Only once the handshake is done: a server may reject notifications sent before it has
        // answered `initialize`.
        if client.state() == vex_lsp::State::Ready {
            if let Some(path) = self.state.file_path.clone() {
                let path = std::path::PathBuf::from(path);
                let text = self.state.document.text().to_string();

                match self.sent_version {
                    None => {
                        let language = vex_lsp::server_for(&path.to_string_lossy())
                            .map(|spec| spec.language_id)
                            .unwrap_or("plaintext");
                        if let Err(error) =
                            client.did_open(&path, language, buffer_version.0 as i32, &text)
                        {
                            eprintln!("vex: didOpen failed: {error:#}");
                        }
                        self.sent_version = Some(buffer_version);
                    }
                    Some(sent) if sent != buffer_version => {
                        if let Err(error) = client.did_change(&path, buffer_version.0 as i32, &text)
                        {
                            eprintln!("vex: didChange failed: {error:#}");
                        }
                        self.sent_version = Some(buffer_version);
                    }
                    _ => {}
                }
            }
        }

        let active_path = self
            .state
            .file_path
            .as_ref()
            .and_then(|path| std::path::Path::new(path).canonicalize().ok());

        let polled = client.poll();
        if !polled.is_empty() && std::env::var_os("VEX_LSP_TRACE").is_some() {
            eprintln!("vex: polled {} lsp events", polled.len());
        }
        for event in polled {
            changed = true;
            match event {
                vex_lsp::ServerEvent::Ready { server_name } => {
                    eprintln!("vex: {server_name} ready");
                    self.state.server_status = server_name;
                    // The handshake finished after the last send, so open the document now.
                    self.sent_version = None;
                }
                vex_lsp::ServerEvent::Diagnostics {
                    path,
                    version,
                    diagnostics,
                } => {
                    // Only for the file on screen: a server reports for the whole project while it
                    // indexes.
                    let matches_active = active_path.as_deref() == Some(path.as_path());
                    if !matches_active && std::env::var_os("VEX_LSP_TRACE").is_some() {
                        eprintln!(
                            "vex: diagnostics ignored — {} is not the active {:?}",
                            path.display(),
                            active_path
                        );
                    }
                    if matches_active {
                        let applied =
                            self.state
                                .diagnostics
                                .apply(diagnostics, version, buffer_version);
                        if applied {
                            let (errors, warnings) = self.state.diagnostics.counts();
                            eprintln!(
                                "vex: diagnostics — {errors} errors, {warnings} warnings (v{})",
                                version.unwrap_or(buffer_version.0 as i32)
                            );
                            self.state.diagnostics_version = vex_text::Version(
                                version.unwrap_or(buffer_version.0 as i32) as u64,
                            );
                        } else if std::env::var_os("VEX_LSP_TRACE").is_some() {
                            eprintln!("vex: diagnostics refused as stale");
                        }
                    }
                }
                vex_lsp::ServerEvent::Message { text } => eprintln!("vex: {text}"),
                vex_lsp::ServerEvent::Stopped => {
                    self.state.server_status = "server stopped".into();
                    self.lsp = None;
                    break;
                }
                // Hover, definition and completion are requested by actions that do not exist yet;
                // the plumbing is in place and unused rather than half-wired into the UI.
                _ => {}
            }
        }

        changed
    }

    /// Whether any background service still owes the editor an answer.
    ///
    /// This is what keeps the event loop alive without spinning: `ControlFlow::Wait` sleeps until
    /// input arrives, which is right for an editor — but a language server that finishes its
    /// handshake while the user is reading has nobody to tell. Without a wake-up, `didOpen` is never
    /// sent and diagnostics never arrive, which is exactly the bug this fixes.
    fn services_busy(&self) -> bool {
        if self.walk.is_some() {
            return true;
        }
        // A streaming search must keep being polled or its hits never appear.
        if self.state.results.as_ref().is_some_and(|r| !r.finished) {
            return true;
        }
        if let Some(client) = &self.lsp {
            // A live server always warrants a slow poll: replies arrive unprompted.
            return client.state() != vex_lsp::State::Stopped;
        }
        self.syntax
            .as_ref()
            .map(|worker| worker.latest().is_none())
            .unwrap_or(false)
    }

    /// Writes a recovery copy of the active buffer if it is dirty.
    ///
    /// Throttled by `recovery::INTERVAL`, so this is safe to call on every keystroke; `force` is for
    /// the moments where losing anything is unacceptable.
    fn persist_recovery(&mut self, force: bool) {
        if !self.state.document.is_dirty() {
            return;
        }
        let Some(path) = self.state.file_path.clone() else {
            // A scratch buffer has no identity to recover to. Rather than invent one, it is left
            // alone — and that limitation is stated in the plan rather than hidden.
            return;
        };
        let text = self.state.document.text().to_string();
        self.recovery
            .maybe_write(std::path::Path::new(&path), &text, force);
    }

    /// Offers unsaved work from a previous session.
    ///
    /// The copy is loaded into the buffer and left dirty, so the next `:w` writes it. Silently
    /// discarding it would defeat the purpose; silently *saving* it would overwrite a file the user
    /// may not want changed.
    fn restore_recovery(&mut self) {
        let Some(path) = self.state.file_path.clone() else {
            return;
        };
        let Some(found) = self.recovery.find(std::path::Path::new(&path)) else {
            return;
        };

        eprintln!(
            "vex: recovered unsaved changes for {path} from {} — the buffer is dirty, :w to keep them",
            found.recovery.display()
        );
        self.state.document = Document::from_str(&found.text);
        // Mark it modified by touching the document through its own edit path would add an undo
        // step; instead the flag is set directly, since the *file* genuinely differs from the buffer.
        self.state.document.mark_dirty();
        self.state.server_status = "recovered unsaved changes".into();
    }

    /// Collects the file walk when it finishes.
    fn poll_walk(&mut self) -> bool {
        let Some(walk) = &self.walk else {
            return false;
        };
        if let Some(result) = walk.poll() {
            let count = result.files.len();
            self.index.set_files(result.files, result.truncated);
            self.walk = None;
            eprintln!(
                "vex: indexed {count} files{}",
                if self.index.is_truncated() {
                    " (truncated)"
                } else {
                    ""
                }
            );
            return true;
        }
        false
    }

    fn sync_syntax(&mut self) {
        if self.syntax.is_none() {
            return;
        }

        // Everything that needs `&self` is read before the worker is borrowed mutably.
        let buffer = self.state.document.version();
        let range = self.visible_range();
        let range_moved = self.requested_range.as_ref() != Some(&range);
        let resync = self.state.document.take_needs_resync();
        let changes = self.state.document.take_changes();
        let snapshot = self.state.document.snapshot();

        let worker = self.syntax.as_mut().expect("checked above");

        // One request per applied transaction, in order: tree-sitter's incremental reparse needs
        // each edit in the coordinates it was expressed in.
        for change in changes {
            worker.request(SyntaxRequest {
                text: change.text,
                version: change.version,
                edits: change.edits,
                range: range.clone(),
                resync,
            });
        }

        // Scrolling changes nothing about the text but everything about what needs highlighting,
        // so re-ask when the visible range moves.
        let never_parsed = worker.latest().is_none();
        if range_moved || never_parsed || resync {
            let (text, version) = snapshot;
            worker.request(SyntaxRequest {
                text,
                version,
                edits: Vec::new(),
                range: range.clone(),
                resync: resync || never_parsed,
            });
        }
        // Apply only what is fresh. A stale result is kept for the ledger to report, never
        // painted over text that has since moved.
        worker.poll();
        let version = worker.version().unwrap_or(Version(0));
        let fresh = worker.fresh_for(buffer).map(|result| {
            (
                result.spans.clone(),
                result.node_path.clone(),
                result.has_error,
            )
        });

        self.requested_range = Some(range);
        self.state.syntax_version = version;
        if let Some((spans, node_path, has_error)) = fresh {
            self.state.highlights = spans;
            self.state.node_path = node_path;
            self.state.syntax_error = has_error;
        }
    }

    /// Keys for the `/` and `:` prompt. Returns true if the editor should quit.
    fn handle_prompt_key(&mut self, key: Key) -> bool {
        let Some(prompt) = &mut self.state.prompt else {
            return false;
        };
        // A message from the previous attempt clears as soon as the user types again.
        prompt.message = None;

        match key.code {
            KeyCode::Escape => {
                // Incremental search moved the cursor; cancelling must put it back, or `/` becomes a
                // way to lose your place.
                let origin = prompt.origin;
                let was_search = prompt.is_search();
                self.state.prompt = None;
                if was_search {
                    self.state.search_matches.clear();
                    self.state.document.set_primary_caret(origin);
                }
            }
            KeyCode::Enter => {
                let prompt = self.state.prompt.take().expect("checked above");
                match prompt.kind {
                    PromptKind::Search { .. } => {
                        // Matches stay highlighted after accepting, so `n` has something to move
                        // through and the eye can find the rest.
                        self.state.search_matches = prompt.matches;
                    }
                    PromptKind::Command => return self.run_command(&prompt.input),
                }
            }
            KeyCode::Backspace => {
                prompt.input.pop();
                self.refresh_prompt();
            }
            KeyCode::Char(c) if !key.ctrl && !key.alt => {
                prompt.input.push(c);
                self.refresh_prompt();
            }
            _ => {}
        }
        false
    }

    /// Re-runs an incremental search and moves the cursor to the first match ahead of the origin.
    fn refresh_prompt(&mut self) {
        let Some(prompt) = &self.state.prompt else {
            return;
        };
        if !prompt.is_search() {
            return;
        }

        let query = prompt.input.clone();
        let origin = prompt.origin;
        let forward = prompt.forward();

        let matches = vex_buffer::find_all(
            self.state.document.text(),
            &query,
            &vex_buffer::SearchOptions::default(),
        );

        // Show the search as it is typed: the highlight is the feedback that makes `/` feel live.
        self.state.search_matches = matches.clone();
        if let Some(found) = vex_buffer::find_next(&matches, origin, forward) {
            self.state.document.set_primary_caret(found.start);
        } else {
            self.state.document.set_primary_caret(origin);
        }

        if let Some(prompt) = &mut self.state.prompt {
            prompt.matches = matches;
        }
        self.reveal_cursor();
    }

    /// Runs a `:` command. Returns true if the editor should quit.
    fn run_command(&mut self, input: &str) -> bool {
        match parse_command(input) {
            Command::Write { path } => {
                if let Some(path) = path {
                    self.state.file_path = Some(path.clone());
                    self.state.path = relative_crumbs(&path);
                }
                self.save_active();
            }
            Command::Quit { force } => {
                if self.state.document.is_dirty() && !force {
                    self.warn("unsaved changes — :q! to discard, :wq to save");
                    return false;
                }
                return true;
            }
            Command::WriteQuit => {
                self.save_active();
                return !self.state.document.is_dirty();
            }
            Command::GotoLine(line) => {
                let text = self.state.document.text();
                // Commands are 1-based, offsets are 0-based; the conversion belongs here, once.
                let target = line.saturating_sub(1).min(text.last_line());
                let offset = text.point_to_char(vex_text::Point::new(target, 0));
                self.state.document.set_primary_caret(offset);
                self.reveal_cursor();
            }
            Command::Edit { path } => {
                if path.is_empty() {
                    self.warn("usage: :e <path>");
                } else {
                    self.activate(Target::File(std::path::PathBuf::from(path)));
                }
            }
            Command::Grep { pattern } => {
                if pattern.is_empty() {
                    self.warn("usage: :grep <pattern>");
                } else {
                    self.state.results = Some(vex::results::Results::start(
                        &self.state.project_root,
                        &pattern,
                    ));
                }
            }
            Command::SetOption { name, value } => match name.as_str() {
                "wrap" => {
                    self.state.display.mode = if value {
                        WrapMode::Word
                    } else {
                        WrapMode::None
                    };
                }
                other => self.warn(&format!("unknown option: {other}")),
            },
            Command::Unknown(name) => self.warn(&format!("not a command: {name}")),
        }
        false
    }

    /// Shows a message on the prompt line rather than silently doing nothing.
    fn warn(&mut self, message: &str) {
        let mut prompt = vex::prompt::Prompt::new(PromptKind::Command, self.state.cursor());
        prompt.message = Some(message.to_string());
        self.state.prompt = Some(prompt);
    }

    /// Keys for the results panel. Returns true if the key was consumed.
    fn handle_results_key(&mut self, key: Key) -> bool {
        let Some(results) = &mut self.state.results else {
            return false;
        };

        match key.code {
            KeyCode::Escape => {
                self.state.results = None;
                true
            }
            KeyCode::Enter => {
                let chosen = results
                    .selection()
                    .map(|hit| (hit.path.clone(), hit.line_number));
                self.state.results = None;
                if let Some((path, line)) = chosen {
                    self.activate(Target::File(path));
                    let text = self.state.document.text();
                    let target = (line.saturating_sub(1) as usize).min(text.last_line());
                    let offset = text.point_to_char(vex_text::Point::new(target, 0));
                    self.state.document.set_primary_caret(offset);
                    self.reveal_cursor();
                }
                true
            }
            // Only the navigation keys are consumed, so editing continues to work with the panel
            // open — it is a list beside the editor, not a modal dialog.
            KeyCode::Up => {
                results.move_selection(-1);
                true
            }
            KeyCode::Down => {
                results.move_selection(1);
                true
            }
            _ => false,
        }
    }

    /// Writes the active buffer, keeping the recovery copy in step.
    fn save_active(&mut self) {
        let Some(path) = self.state.file_path.clone() else {
            self.warn("no file name — use :w <path>");
            return;
        };
        let text = self.state.document.text().to_string();

        match std::fs::write(&path, &text) {
            Ok(()) => {
                self.state.document.mark_saved();
                // The file on disk is authoritative now, so the recovery copy must go — otherwise it
                // would be offered again on the next open.
                self.recovery.discard(std::path::Path::new(&path));
                eprintln!("vex: wrote {path}");
            }
            Err(error) => self.warn(&format!("could not write {path}: {error}")),
        }
    }

    fn open_palette(&mut self, kind: PaletteKind) {
        let mut palette = Palette::new(kind);
        match kind {
            PaletteKind::Files => palette.refresh_files(&mut self.index),
            PaletteKind::Buffers => palette.refresh_buffers(&self.buffer_names()),
        }
        self.state.palette = Some(palette);
    }

    /// Names and directories of every open buffer, active one first in its own slot.
    fn buffer_names(&self) -> Vec<(String, String)> {
        let mut names = Vec::with_capacity(self.buffers.len() + 1);
        for index in 0..=self.buffers.len() {
            let (path, _) = self.buffer_at(index);
            let name = path.last().cloned().unwrap_or_default();
            let directory = if path.len() > 1 {
                path[..path.len() - 1].join("/")
            } else {
                String::new()
            };
            names.push((name, directory));
        }
        names
    }

    /// The path of buffer `index`, where `active_buffer` refers to the live document.
    fn buffer_at(&self, index: usize) -> (Vec<String>, bool) {
        if index == self.state.active_buffer {
            return (self.state.path.clone(), true);
        }
        // Buffers are stored in order with the active one's slot skipped.
        let stored = if index < self.state.active_buffer {
            index
        } else {
            index - 1
        };
        match self.buffers.get(stored) {
            Some(buffer) => (buffer.path.clone(), false),
            None => (self.state.path.clone(), true),
        }
    }

    fn handle_palette_key(&mut self, key: Key) {
        let Some(palette) = &mut self.state.palette else {
            return;
        };

        match key.code {
            KeyCode::Escape => self.state.palette = None,
            KeyCode::Enter => {
                let chosen = palette.selection().map(|row| row.target.clone());
                self.state.palette = None;
                if let Some(target) = chosen {
                    self.activate(target);
                }
            }
            KeyCode::Up => palette.move_selection(-1),
            KeyCode::Down => palette.move_selection(1),
            KeyCode::Backspace => {
                palette.query.pop();
                self.refresh_palette();
            }
            KeyCode::Char(c) if !key.ctrl && !key.alt => {
                palette.query.push(c);
                self.refresh_palette();
            }
            _ => {}
        }
    }

    fn refresh_palette(&mut self) {
        let names = self.buffer_names();
        if let Some(palette) = &mut self.state.palette {
            match palette.kind {
                PaletteKind::Files => palette.refresh_files(&mut self.index),
                PaletteKind::Buffers => palette.refresh_buffers(&names),
            }
        }
    }

    /// Opens a file or switches to an already-open buffer.
    fn activate(&mut self, target: Target) {
        match target {
            Target::File(path) => {
                let path_string = path.to_string_lossy().to_string();

                // Already open? Switch rather than loading a second copy, which would give the file
                // two independent undo histories.
                if let Some(index) = (0..=self.buffers.len()).find(|index| {
                    self.buffer_at(*index).0.join("/") == relative_crumbs(&path_string).join("/")
                }) {
                    self.switch_to(index);
                    return;
                }

                match std::fs::read_to_string(&path) {
                    Ok(contents) => self.open_document(
                        Document::from_str(&contents),
                        relative_crumbs(&path_string),
                        Some(path_string),
                    ),
                    Err(err) => eprintln!("vex: could not open {}: {err}", path.display()),
                }
            }
            Target::Buffer(index) => self.switch_to(index),
        }
    }

    /// Makes a new document the active one, keeping the previous buffer.
    fn open_document(&mut self, document: Document, path: Vec<String>, file_path: Option<String>) {
        let previous = Buffer {
            document: std::mem::replace(&mut self.state.document, document),
            path: std::mem::replace(&mut self.state.path, path),
            file_path: std::mem::replace(&mut self.state.file_path, file_path),
            scroll: std::mem::take(&mut self.state.scroll),
        };
        self.buffers.insert(self.state.active_buffer, previous);
        self.state.active_buffer = self.buffers.len();
        self.state.buffer_count = self.buffers.len() + 1;

        self.after_buffer_change();
    }

    fn switch_to(&mut self, index: usize) {
        if index == self.state.active_buffer || index > self.buffers.len() {
            return;
        }

        let stored = if index < self.state.active_buffer {
            index
        } else {
            index - 1
        };
        let Some(target) = self.buffers.get_mut(stored) else {
            return;
        };

        // Swap the live document out and the chosen one in. Both keep their own undo history and
        // scroll position, which is what makes switching back feel like returning rather than
        // reopening.
        std::mem::swap(&mut self.state.document, &mut target.document);
        std::mem::swap(&mut self.state.path, &mut target.path);
        std::mem::swap(&mut self.state.file_path, &mut target.file_path);
        std::mem::swap(&mut self.state.scroll, &mut target.scroll);

        self.state.active_buffer = index;
        self.after_buffer_change();
    }

    /// Resets per-buffer derived state after the active document changes.
    fn after_buffer_change(&mut self) {
        // Highlights belong to the old document; keeping them would paint one file's colors over
        // another's text until the next parse landed.
        self.state.highlights.clear();
        self.state.node_path.clear();
        self.state.syntax_error = false;
        self.state.syntax_version = Version(0);
        self.requested_range = None;

        // The language may differ, so the worker is restarted for the new file.
        self.syntax = None;
        if let Some(language) = self
            .state
            .file_path
            .as_deref()
            .and_then(Language::from_path)
        {
            match SyntaxWorker::spawn(language) {
                Ok(worker) => self.syntax = Some(worker),
                Err(err) => eprintln!("vex: syntax unavailable: {err}"),
            }
        }

        // Drain the change queue: the new document's changes are not the old one's.
        self.state.document.take_changes();
        self.sync_syntax();

        // A different file may need a different server.
        self.start_language_server();
        self.sync_lsp();
    }

    fn redraw(&mut self) {
        // Also here, so the index and any server replies land without waiting for a keystroke.
        self.poll_walk();
        self.sync_lsp();

        let Some(renderer) = &mut self.renderer else {
            return;
        };

        let (width, height) = renderer.size();
        let scale = self
            .window
            .as_ref()
            .map(|window| window.scale_factor() as f32)
            .unwrap_or(1.0);

        // Read the previous frame's timings before drawing this one, so the number on screen is
        // a measurement rather than a prediction.
        self.state.frame_ms = renderer.timer.average_ms();
        self.state.worst_ms = renderer.timer.worst_ms();

        self.frame.clear();
        chrome::draw(
            &mut self.frame,
            &self.state,
            Rect::new(0.0, 0.0, width as f32, height as f32),
            scale,
        );

        if let Err(err) = renderer.render(&self.frame, self.state.theme.bg) {
            eprintln!("vex: frame dropped: {err:#}");
        }
    }
}

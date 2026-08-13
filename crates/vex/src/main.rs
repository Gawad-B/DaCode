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
use vex::panes::Panes;
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

    let (state, keymap) = new_state(text, crumbs, file_path, Mode::Normal, None, root.clone());
    let mut grammar = Grammar::new();
    grammar.set_keymap(keymap);

    let mut app = App {
        window: None,
        renderer: None,
        frame: Frame::default(),
        modifiers: ModifiersState::empty(),
        grammar,
        registers: Registers::new(),
        last_change: None,
        syntax: None,
        requested_range: None,
        lsp: None,
        sent_version: None,
        recovery: Recovery::new(),
        watcher: match vex_project::FileWatcher::new() {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                // Not fatal: the editor works, it just will not notice outside edits.
                eprintln!("vex: file watching unavailable: {error}");
                None
            }
        },
        self_written: std::collections::HashSet::new(),
        panes: Panes::new(),
        edit_version: None,
        save_after_format: false,
        wakes: 0,
        index: FileIndex::new(root.clone()),
        walk: Some(FileWalk::start(root.clone())),
        buffers: Vec::new(),
        state,
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
) -> (EditorState, vex_input::Keymap) {
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

    // Configuration problems are shown in the info box, which is dismissible with escape like any
    // other. Printing them to stderr only would mean a mistyped binding just quietly does nothing —
    // and the editor is usually not started from a terminal anyone is watching.
    let notice = (!settings.problems.is_empty()).then(|| {
        for problem in &settings.problems {
            eprintln!("vex: config: {problem}");
        }
        let count = settings.problems.len();
        let noun = if count == 1 { "problem" } else { "problems" };
        std::iter::once(format!("config: {count} {noun}"))
            .chain(settings.problems.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    });

    let state = EditorState {
        mode,
        theme: theme_override.unwrap_or(settings.theme),
        typography: settings.typography,
        document: Document::from_text(text),
        pairs: pair_config(file_path.as_deref(), settings.tab_width),
        format_on_save: settings.format_on_save,
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
        hover: notice,
        completion: None,
        pending_request: None,
        project_root,
        buffer_count: 1,
        active_buffer: 0,
        tree: None,
    };

    (state, settings.keymap)
}

/// Pairing and indentation rules for a file.
///
/// Per-language, because the differences matter: Python indents after `:` and has no braces to
/// balance, while HTML and CSS should not pair quotes the way a programming language does.
fn pair_config(path: Option<&str>, tab_width: usize) -> vex_buffer::PairConfig {
    use vex_syntax::Language;

    let mut config = vex_buffer::PairConfig {
        tab_width,
        ..Default::default()
    };

    match path.and_then(Language::from_path) {
        Some(Language::Python) => {
            // A colon opens a block; braces are dictionaries and still worth pairing.
            config.block_openers = vec![':', '{', '(', '['];
        }
        Some(Language::Rust) => {
            // `'` is a lifetime far more often than a character literal, and pairing it is a
            // constant irritation. `on_type` already refuses mid-word; this covers the rest.
            config.quotes = vec!['"'];
        }
        Some(Language::Html) => {
            // Angle brackets are matched by the language, not by us: pairing `<` would fight every
            // comparison and generic in embedded script.
            config.quotes = vec!['"', '\''];
        }
        _ => {}
    }

    config
}

/// Flags that consume the argument after them. Without this list, a flag's *value* gets mistaken
/// for the file to open — which is how `--columns 60` ended up trying to open a file named `60`.
const VALUE_FLAGS: [&str; 9] = [
    "--screenshot",
    "--mode",
    "--columns",
    "--palette",
    "--search",
    "--grep",
    "--diagnostic",
    "--complete",
    "--split",
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
/// The file the screenshot fixtures open. A real path in this repo, so the captures show real code
/// with real syntax highlighting rather than a lorem-ipsum buffer that could hide a defect.
const FIXTURE: &str = "crates/vex-text/src/anchor.rs";

fn load(path: Option<String>) -> Result<(Text, Vec<String>, Option<String>)> {
    // No argument means no buffer: the editor opens empty and says how to open something. Until now
    // it opened a file from this repository, which was a development scaffold from the first
    // milestone and not behavior anyone would want.
    let Some(path) = path else {
        return Ok((Text::from_str(""), vec!["untitled".to_string()], None));
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let crumbs = path.split('/').map(str::to_string).collect();
            Ok((Text::from_str(&contents), crumbs, Some(path)))
        }
        // A missing file is a scratch buffer, not a fatal error — but it must not be silently
        // saveable over some other path, so `file_path` stays `None`.
        Err(err) => {
            eprintln!("vex: could not read {path}: {err} — opening an empty buffer");
            Ok((Text::from_str(""), vec!["untitled".to_string()], None))
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

    // The fixture is explicit here rather than being a default inside `load`: the editor proper
    // opens empty, and the captures need content.
    let opened = match (positional(args), flag("--empty")) {
        (_, true) => None,
        (explicit, false) => explicit.or_else(|| Some(FIXTURE.to_string())),
    };
    let (text, crumbs, file_path) = load(opened)?;

    let (width, height) = (1600u32, 1000u32);
    let root = find_root(&std::path::PathBuf::from("."));
    let (mut state, _keymap) = new_state(text, crumbs, file_path, mode, theme, root);

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

    // `--diagnostic LINE:COL:MESSAGE` injects a diagnostic so the underline and message box can be
    // checked without waiting on a language server.
    if let Some(spec) = value("--diagnostic") {
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        if parts.len() == 3 {
            let line: u32 = parts[0].parse().unwrap_or(0);
            let column: u32 = parts[1].parse().unwrap_or(0);
            let end = column + 12;
            state.diagnostics.apply(
                vec![vex_lsp::lsp_types::Diagnostic {
                    range: vex_lsp::lsp_types::Range {
                        start: vex_lsp::lsp_types::Position {
                            line,
                            character: column,
                        },
                        end: vex_lsp::lsp_types::Position {
                            line,
                            character: end,
                        },
                    },
                    severity: Some(vex_lsp::lsp_types::DiagnosticSeverity::ERROR),
                    message: parts[2].to_string(),
                    ..Default::default()
                }],
                None,
                state.document.version(),
            );
            // The cursor is parked a few lines away so the underline renders unobstructed; the
            // message box only appears when the cursor is *on* the diagnostic, which is checked
            // separately.
            let text = state.document.text();
            let away = (line as usize + 3).min(text.last_line());
            let target = text.point_to_char(vex_text::Point::new(away, 0));
            state.document.set_primary_caret(target);
        }
    }

    // `--complete PREFIX` opens a completion popup with a canned list, so the popup is inspectable
    // without a language server.
    if let Some(prefix) = value("--complete") {
        // Typed into the buffer rather than handed to the session: the popup is a function of the
        // text before the caret, so faking the prefix any other way would not exercise the filter.
        // At the end of the line and after a space, because a prefix typed into the middle of an
        // identifier would extend *that* word and match nothing.
        state.document.move_to_line_edge(true, false);
        state.document.insert(&format!(" {prefix}"));
        let caret = state.cursor();
        let mut session = vex::completion::Completion::new(
            state.document.text(),
            caret,
            state.document.version(),
        );
        session.offer(
            vec![
                "char_to_point".into(),
                "char_to_byte".into(),
                "char_to_line".into(),
                "chars".into(),
                "clamp".into(),
                "clone".into(),
            ],
            state.document.text(),
            caret,
        );
        state.completion = Some(session);
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

    let bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
    let mut frame = Frame::default();
    if flag("--splash") {
        chrome::draw_splash(&mut frame, &state.theme, &state.typography, bounds, 1.0);
    } else if flag("--tree") {
        // The tree needs a file list; the screenshot path has no background walk, so index inline.
        let root = find_root(&std::path::PathBuf::from("."));
        let walk = FileWalk::start(root.clone());
        let mut index = FileIndex::new(root.clone());
        for _ in 0..200 {
            if let Some(result) = walk.poll() {
                index.set_files(result.files, result.truncated);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let mut tree = vex::tree::FileTree::new(root, index.files());
        tree.reveal("crates/vex-text/src/anchor.rs");
        state.tree = Some(tree);
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if let Some(split) = value("--split") {
        // `--split N[h]` renders N panes on one buffer, so the layout is inspectable without a
        // window. The panes look at different places in the file, which is the case worth seeing:
        // identical panes would hide a scroll bug.
        let horizontal = split.ends_with('h');
        let count = split
            .trim_end_matches(['h', 'v'])
            .parse::<usize>()
            .unwrap_or(2)
            .clamp(1, 4);

        let panes: Vec<chrome::PaneView<'_>> = (0..count)
            .map(|index| chrome::PaneView {
                document: &state.document,
                scroll: ScrollAnchor {
                    line: index * 14,
                    sub_row: 0,
                },
                path: &state.path,
                focused: index == 0,
                highlights: &state.highlights,
            })
            .collect();

        chrome::draw_panes(
            &mut frame,
            &state,
            &panes,
            if horizontal {
                chrome::PaneLayout::Horizontal
            } else {
                chrome::PaneLayout::Vertical
            },
            bounds,
            1.0,
        );
    } else {
        chrome::draw(&mut frame, &state, bounds, 1.0);
    }

    headless
        .capture_png(&frame, state.theme.bg, output)
        .with_context(|| format!("writing {output}"))?;
    eprintln!(
        "vex: wrote {output} — {} quads, {} text runs, {} marks",
        frame.quads.len(),
        frame.texts.len(),
        frame.marks.len()
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
    /// Set when a save asked for formatting: the reply's edits are applied and the file written
    /// again. The first write already happened — a save must land on disk when asked, not when a
    /// language server gets around to answering.
    save_after_format: bool,
    /// The version an outstanding edit request was computed against.
    ///
    /// Rename and formatting replies carry positions into a specific version of the document. If the
    /// user typed while the server was thinking, applying them would corrupt the file, so the reply
    /// is refused — see `edits::apply_to_document`.
    edit_version: Option<vex_text::Version>,
    /// Persists dirty buffers so a crash loses seconds, not work.
    recovery: Recovery,
    /// Watches open files for changes made outside the editor.
    watcher: Option<vex_project::FileWatcher>,
    /// Paths this editor is about to write, so its own saves are not mistaken for someone else's
    /// edit. Without this, every save would come straight back as "changed on disk".
    self_written: std::collections::HashSet<std::path::PathBuf>,
    /// Counts event-loop wake-ups, for tracing only.
    wakes: u64,
    /// Open panes, and which has focus. Always at least one — see `vex::panes`.
    panes: Panes,
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
    /// The highlights this buffer had when it was last live.
    ///
    /// Kept so a pane showing this buffer can be drawn with its colors while another pane has focus.
    /// They stay correct because an unfocused buffer cannot be edited — keys only reach the focused
    /// pane — and the moment it is edited it is the live buffer again.
    highlights: Vec<vex_syntax::HighlightSpan>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attributes = Window::default_attributes()
            .with_title("vex")
            .with_window_icon(vex::icon::window_icon())
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 820.0));

        // The window has to name itself, or the desktop cannot connect it to its installed entry.
        //
        // Wayland has no window icon protocol at all — `with_window_icon` above is ignored there — so
        // the *only* way GNOME finds an icon for a running window is by matching its `app_id` against
        // an installed `.desktop` file. `vex` matches `vex.desktop`. On X11 the same string is the
        // WM_CLASS that `StartupWMClass=vex` in that file refers to. Without this, the running editor
        // shows a generic placeholder in the dock and the alt-tab switcher even though the icon is
        // correctly installed.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            attributes = WindowAttributesExtWayland::with_name(attributes, "vex", "vex");
            attributes = WindowAttributesExtX11::with_name(attributes, "vex", "vex");
        }

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("vex: could not create a window: {err}");
                event_loop.exit();
                return;
            }
        };

        // The file opened at startup never went through `after_buffer_change`, so this is where it
        // starts being watched. Missing it meant the first file you opened — the one you are most
        // likely to be editing — was the one file changes were not noticed for.
        self.watch_active();

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
        changed |= self.handle_file_changes();
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
                        self.refresh_completion();
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
        // A hover box is dismissed by the next Escape, before it reaches the grammar and collapses
        // selections — one Escape, one effect.
        if self.state.hover.is_some() && key.code == KeyCode::Escape {
            self.state.hover = None;
            return false;
        }
        if self.state.completion.is_some() && self.handle_completion_key(key) {
            return false;
        }
        if self.state.results.is_some() && self.handle_results_key(key) {
            return false;
        }

        if self.handle_tree_key(key) {
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
        if let Outcome::Window(action) = &outcome {
            self.window_action(action);
        }

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

        // Requests the executor recorded: sent here, where the client lives.
        if let Some(request) = self.state.pending_request.take() {
            if client.state() == vex_lsp::State::Ready {
                if let Some(path) = self.state.file_path.clone() {
                    let path = std::path::PathBuf::from(path);
                    let position = vex_lsp::to_lsp(self.state.document.text(), self.state.cursor());
                    let sent = match &request {
                        Action::Hover => client.hover(&path, position),
                        Action::GotoDefinition => client.definition(&path, position),
                        Action::RequestCompletion => client.completion(&path, position),
                        Action::Format => {
                            // The version the reply will be checked against: edits computed for a
                            // document the user has since typed into are refused, not merged.
                            self.edit_version = Some(self.state.document.version());
                            client.formatting(&path, self.state.display.tab_width)
                        }
                        Action::Rename => Ok(0),
                        _ => Ok(0),
                    };
                    if let Err(error) = sent {
                        eprintln!("vex: request failed: {error:#}");
                    }
                }
            } else {
                // Saying so beats a keypress that appears to do nothing while the server starts.
                self.state.hover = Some(format!("{} is still starting", self.state.server_status));
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
                vex_lsp::ServerEvent::Completion { items } => {
                    let caret = self.state.cursor();
                    let text = self.state.document.text().clone();
                    if let Some(completion) = &mut self.state.completion {
                        completion.offer(items, &text, caret);
                    }
                }
                vex_lsp::ServerEvent::Hover { text } => {
                    self.state.hover = Some(if text.trim().is_empty() {
                        "no information here".to_string()
                    } else {
                        text
                    });
                }
                vex_lsp::ServerEvent::Definition { path, position } => {
                    self.jump_to(path, position);
                }
                vex_lsp::ServerEvent::Rename { new_name, edit } => {
                    self.apply_server_edit(
                        &format!("renamed to `{new_name}`"),
                        |state, version| {
                            let path = state.file_path.clone().map(std::path::PathBuf::from);
                            vex::edits::apply_workspace_edit(
                                &mut state.document,
                                path.as_deref(),
                                &edit,
                                version,
                            )
                        },
                    );
                }
                vex_lsp::ServerEvent::Formatting { edits } => {
                    self.apply_server_edit("formatted", |state, version| {
                        vex::edits::apply_to_document(&mut state.document, &edits, version).map(
                            |count| vex::edits::Applied {
                                in_buffer: count,
                                files: Vec::new(),
                            },
                        )
                    });
                }
                vex_lsp::ServerEvent::RequestFailed {
                    what,
                    reason,
                    retry,
                } => {
                    // On screen, not just on stderr: this is something the user asked for and did not
                    // get. Retryable failures say so — a language server that is still indexing will
                    // answer the same question happily a second later, and "try again" is the whole
                    // fix. Not retried automatically: a request that fires itself again without being
                    // asked is how an editor ends up hammering a busy server.
                    let message = if retry {
                        format!("{what}: the server is busy — try again")
                    } else {
                        format!("{what} failed: {reason}")
                    };
                    eprintln!("vex: {message}");
                    self.state.hover = Some(message);
                    self.edit_version = None;
                    self.save_after_format = false;
                }
                vex_lsp::ServerEvent::Message { text } => eprintln!("vex: {text}"),
                vex_lsp::ServerEvent::Stopped => {
                    self.state.server_status = "server stopped".into();
                    self.lsp = None;
                    break;
                }
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

    /// Keys the completion popup consumes. Returns true if the key was used.
    ///
    /// Only the keys that mean something to a popup are taken: everything else — including ordinary
    /// characters — falls through to the editor, which then refilters. Swallowing every key would
    /// make the popup a modal dialog, and completion is not one.
    fn handle_completion_key(&mut self, key: Key) -> bool {
        let Some(completion) = &mut self.state.completion else {
            return false;
        };

        match key.code {
            KeyCode::Escape => {
                self.state.completion = None;
                true
            }
            KeyCode::Up => {
                completion.move_selection(-1);
                true
            }
            KeyCode::Down => {
                completion.move_selection(1);
                true
            }
            KeyCode::Tab | KeyCode::Enter => {
                let chosen = completion.selection().map(|item| item.label.clone());
                let anchor = completion.anchor;
                self.state.completion = None;

                match chosen {
                    Some(label) => {
                        // Replace the typed prefix, so accepting `length` after typing `len` does not
                        // produce `lenlength`.
                        let caret = self.state.cursor();
                        let edit = vex_text::Edit::replace(anchor..caret, label);
                        let _ = self.state.document.apply(
                            vex_text::Transaction::new(vec![edit]),
                            vex_buffer::EditKind::Discrete,
                        );
                        true
                    }
                    // Nothing selected: let Enter do what Enter does.
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Keeps an open completion session in step with what has been typed.
    fn refresh_completion(&mut self) {
        let Some(completion) = &self.state.completion else {
            return;
        };
        let caret = self.state.cursor();

        if !completion.is_valid(caret) {
            self.state.completion = None;
            return;
        }

        let text = self.state.document.text().clone();
        if let Some(completion) = &mut self.state.completion {
            completion.refilter(&text, caret);
            // A session with nothing left to offer is noise.
            if !completion.pending && completion.items.is_empty() {
                self.state.completion = None;
            }
        }
    }

    /// Opens a file (if needed) and puts the cursor on an LSP position.
    ///
    /// Used by goto-definition, where the target is frequently in another file — and where landing on
    /// the wrong line would be worse than not jumping at all, so the position is converted through
    /// the same UTF-16 aware function as everything else.
    fn jump_to(&mut self, path: std::path::PathBuf, position: vex_lsp::lsp_types::Position) {
        let already_here = self
            .state
            .file_path
            .as_deref()
            .map(std::path::Path::new)
            .and_then(|current| current.canonicalize().ok())
            == path.canonicalize().ok();

        if !already_here {
            self.activate(Target::File(path));
        }

        let offset = vex_lsp::from_lsp(self.state.document.text(), position);
        self.state.document.set_primary_caret(offset);
        self.state.hover = None;
        self.reveal_cursor();
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

            // The walk finishes after the tree can first be opened on a large project, so the tree
            // is refreshed rather than left showing whatever had been indexed at that moment.
            if let Some(tree) = &mut self.state.tree {
                tree.refresh(self.index.files());
            }
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
                    PromptKind::Rename => {
                        let name = prompt.input.trim().to_string();
                        if name.is_empty() {
                            self.warn("rename cancelled: no name given");
                        } else {
                            self.request_rename(&name);
                        }
                    }
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
            Command::Format => self.state.pending_request = Some(Action::Format),
            Command::Replace {
                pattern,
                replacement,
            } => self.replace_in_results(&pattern, &replacement),
            Command::Rename { new_name } => match new_name {
                Some(name) => self.request_rename(&name),
                // Without a name, open the prompt — pre-filled, exactly as `gr` does. Routed through
                // the executor so there is one definition of what a rename prompt looks like.
                None => {
                    execute(&mut self.state, &mut self.registers, &Action::Rename, 1);
                }
            },
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
}

/// Borrows each pane's buffer for drawing.
///
/// A free function over the individual fields rather than a method, because the caller holds
/// `&mut self.frame` and `&mut self.renderer` at the same time — borrowing fields separately is what
/// lets those coexist.
fn pane_views<'a>(
    panes: &'a Panes,
    state: &'a EditorState,
    buffers: &'a [Buffer],
) -> Vec<chrome::PaneView<'a>> {
    let active_pane = panes.active();
    panes
        .iter()
        .enumerate()
        .map(|(index, pane)| {
            let focused = index == active_pane;
            let live = pane.buffer == state.active_buffer;
            if live {
                chrome::PaneView {
                    document: &state.document,
                    // The focused pane's scroll is the live one; another pane on the same buffer
                    // keeps its own place in it.
                    scroll: if focused { state.scroll } else { pane.scroll },
                    path: &state.path,
                    focused,
                    highlights: &state.highlights,
                }
            } else {
                let stored = stored_index(pane.buffer, state.active_buffer);
                match stored.and_then(|index| buffers.get(index)) {
                    Some(buffer) => chrome::PaneView {
                        document: &buffer.document,
                        scroll: pane.scroll,
                        path: &buffer.path,
                        focused,
                        highlights: &buffer.highlights,
                    },
                    // The buffer went away — a pane pointing at nothing draws the live one
                    // rather than vanishing.
                    None => chrome::PaneView {
                        document: &state.document,
                        scroll: pane.scroll,
                        path: &state.path,
                        focused,
                        highlights: &state.highlights,
                    },
                }
            }
        })
        .collect()
}

/// Where a buffer index lives in `buffers`, which excludes the active one.
fn stored_index(buffer: usize, active: usize) -> Option<usize> {
    if buffer == active {
        return None;
    }
    Some(if buffer < active { buffer } else { buffer - 1 })
}

impl App {
    /// Carries out a window action.
    fn window_action(&mut self, action: &Action) {
        match action {
            Action::SplitPane { vertical } => {
                self.save_pane();
                self.panes.split(if *vertical {
                    chrome::PaneLayout::Vertical
                } else {
                    chrome::PaneLayout::Horizontal
                });
            }
            Action::ClosePane => {
                if self.panes.close() {
                    self.restore_pane();
                }
            }
            Action::ToggleTree => {
                if self.state.tree.is_some() {
                    self.state.tree = None;
                } else {
                    let mut tree = vex::tree::FileTree::new(
                        self.index.root().to_path_buf(),
                        self.index.files(),
                    );
                    // Open onto the file being edited rather than a collapsed root: the tree is a
                    // way to look around from where you are, not a fresh start.
                    if let Some(path) = &self.state.file_path {
                        if let Ok(absolute) = std::path::Path::new(path).canonicalize() {
                            if let Ok(relative) = absolute.strip_prefix(self.index.root()) {
                                tree.reveal(&relative.to_string_lossy());
                            }
                        }
                    }
                    self.state.tree = Some(tree);
                }
            }
            Action::FocusPane { forward } if self.panes.is_split() => {
                self.save_pane();
                self.panes.cycle(*forward);
                self.restore_pane();
            }
            _ => {}
        }
    }

    /// Records where the focused pane is looking, before focus moves away from it.
    fn save_pane(&mut self) {
        let buffer = self.state.active_buffer;
        let scroll = self.state.scroll;
        let selections = self.state.document.selections().clone();
        let pane = self.panes.focused_mut();
        pane.buffer = buffer;
        pane.scroll = scroll;
        pane.selections = selections;
    }

    /// Brings the newly focused pane's buffer, scroll and selections back to the live state.
    fn restore_pane(&mut self) {
        let pane = self.panes.focused().clone();
        if pane.buffer != self.state.active_buffer {
            self.switch_to(pane.buffer);
        }
        self.state.scroll = pane.scroll;
        self.state.document.set_selections(pane.selections);
    }

    /// Sends a rename request for the symbol under the cursor.
    fn request_rename(&mut self, new_name: &str) {
        let Some(path) = self.state.file_path.clone() else {
            self.warn("rename needs a saved file");
            return;
        };
        let Some(client) = &self.lsp else {
            self.warn("rename needs a language server");
            return;
        };
        if client.state() != vex_lsp::State::Ready {
            self.warn(&format!("{} is still starting", self.state.server_status));
            return;
        }

        let position = vex_lsp::to_lsp(self.state.document.text(), self.state.cursor());
        self.edit_version = Some(self.state.document.version());
        if let Err(error) = client.rename(std::path::Path::new(&path), position, new_name) {
            self.warn(&format!("rename failed: {error:#}"));
        }
    }

    /// Applies edits a server sent, and says what happened either way.
    ///
    /// The version check lives here rather than in each caller so that no reply path can forget it.
    fn apply_server_edit(
        &mut self,
        what: &str,
        apply: impl FnOnce(
            &mut EditorState,
            Option<vex_text::Version>,
        ) -> Result<vex::edits::Applied, vex::edits::EditError>,
    ) {
        let version = self.edit_version.take();
        match apply(&mut self.state, version) {
            Ok(applied) if applied.total() == 0 => {
                // Already formatted: the file on disk is correct, so there is nothing to write.
                self.save_after_format = false;
                self.state.hover = Some(format!("{what}: nothing to change"));
            }
            Ok(applied) => {
                // Files changed outside the buffer are already on disk; the syntax tree and the
                // server's copy of this buffer both need the new text.
                self.state.document.mark_dirty();
                self.sync_syntax();
                self.state.hover = Some(applied.summary(what));
                if std::mem::take(&mut self.save_after_format) {
                    self.save_active();
                }
                if !applied.files.is_empty() {
                    let names: Vec<String> = applied
                        .files
                        .iter()
                        .map(|(path, count)| format!("{} ({count})", path.display()))
                        .collect();
                    eprintln!("vex: {what} — also wrote {}", names.join(", "));
                }
                self.reveal_cursor();
            }
            Err(error) => {
                self.save_after_format = false;
                eprintln!("vex: {what} failed: {error}");
                self.state.hover = Some(format!("{what} failed: {error}"));
            }
        }
    }

    /// Replaces across the files the open search found.
    ///
    /// Deliberately scoped to the results panel rather than the whole project: you replace in what
    /// you can see, having read it. A replace that also touched files never listed would be a much
    /// more dangerous operation wearing the same name.
    fn replace_in_results(&mut self, pattern: &str, replacement: &str) {
        let Some(results) = &self.state.results else {
            self.warn("replace needs search results — run :grep or ctrl-f first");
            return;
        };
        if results.hits.is_empty() {
            self.warn("nothing to replace: the search found no matches");
            return;
        }
        if !results.finished {
            self.warn("the search is still running — wait for it to finish");
            return;
        }

        let hits = results.hits.clone();
        let path = self.state.file_path.clone().map(std::path::PathBuf::from);

        match vex::edits::replace_in_hits(
            &mut self.state.document,
            path.as_deref(),
            &hits,
            pattern,
            replacement,
        ) {
            Ok(applied) if applied.total() == 0 => self.warn("nothing matched"),
            Ok(applied) => {
                let summary = applied.summary(&format!("replaced `{pattern}`"));
                eprintln!("vex: {summary}");
                self.state.hover = Some(summary);
                self.sync_syntax();
                // The results describe text that no longer exists, so the panel closes rather than
                // listing hits that would jump to the wrong place.
                self.state.results = None;
            }
            Err(error) => self.warn(&format!("replace failed: {error}")),
        }
    }

    /// Shows a message on the prompt line rather than silently doing nothing.
    fn warn(&mut self, message: &str) {
        let mut prompt = vex::prompt::Prompt::new(PromptKind::Command, self.state.cursor());
        prompt.message = Some(message.to_string());
        self.state.prompt = Some(prompt);
    }

    /// Keys for the file tree. Returns true if the key was consumed.
    ///
    /// The tree only takes keys that mean something in it, so `ctrl-p`, `:` and the editing keys all
    /// keep working with it open — it is a panel beside the text, not a mode.
    fn handle_tree_key(&mut self, key: Key) -> bool {
        let Some(tree) = &mut self.state.tree else {
            return false;
        };
        if key.ctrl || key.alt {
            return false;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => tree.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => tree.move_selection(-1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if let Some(path) = tree.activate() {
                    self.activate(Target::File(path));
                }
            }
            KeyCode::Escape => self.state.tree = None,
            _ => return false,
        }
        true
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

        if let Ok(absolute) = std::path::Path::new(&path).canonicalize() {
            self.self_written.insert(absolute);
        }

        match std::fs::write(&path, &text) {
            Ok(()) => {
                // Canonicalization only works once the file exists, so a first save records it here.
                if let Ok(absolute) = std::path::Path::new(&path).canonicalize() {
                    self.self_written.insert(absolute);
                }
                self.state.document.mark_saved();
                // The file on disk is authoritative now, so the recovery copy must go — otherwise it
                // would be offered again on the next open.
                self.recovery.discard(std::path::Path::new(&path));
                eprintln!("vex: wrote {path}");

                // Formatting is a round trip, so it cannot come *before* the write without either
                // blocking the UI thread or risking a save that never happens. The file is saved as
                // asked, then reformatted and saved again when the reply arrives.
                if self.state.format_on_save
                    && self
                        .lsp
                        .as_ref()
                        .is_some_and(|client| client.state() == vex_lsp::State::Ready)
                {
                    self.save_after_format = true;
                    self.state.pending_request = Some(Action::Format);
                }
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
            highlights: std::mem::take(&mut self.state.highlights),
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
        // Highlights travel with the buffer so an unfocused pane keeps its colors.
        std::mem::swap(&mut self.state.highlights, &mut target.highlights);

        self.state.active_buffer = index;
        self.after_buffer_change();
    }

    /// Starts watching the active file, if it has one.
    fn watch_active(&mut self) {
        let Some(path) = self.state.file_path.clone() else {
            return;
        };
        if let Some(watcher) = &mut self.watcher {
            if let Err(error) = watcher.watch(std::path::Path::new(&path)) {
                eprintln!("vex: could not watch {path}: {error}");
            }
        }
    }

    /// Handles files that changed outside the editor.
    ///
    /// The rule is that unsaved work is never discarded. A clean buffer is reloaded silently, which
    /// is what makes `git checkout` and a formatter run in a terminal behave the way you expect. A
    /// buffer with unsaved changes is *not* touched — it says so instead, and the choice of whose
    /// version wins stays with the person who made both.
    fn handle_file_changes(&mut self) -> bool {
        let Some(watcher) = &self.watcher else {
            return false;
        };
        let changed = watcher.poll();
        if changed.is_empty() {
            return false;
        }

        let mut redraw = false;
        for path in changed {
            // A save this editor just made comes back as an event; ignoring it once is the whole
            // difference between a quiet save and a reload storm.
            if self.self_written.remove(&path) {
                continue;
            }

            let active = self
                .state
                .file_path
                .as_ref()
                .and_then(|open| std::path::Path::new(open).canonicalize().ok());
            if active.as_deref() != Some(path.as_path()) {
                continue;
            }

            if self.state.document.is_dirty() {
                self.state.hover = Some(format!(
                    "{} changed on disk, and this buffer has unsaved changes — :e to discard yours, \
                     :w to overwrite theirs",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                redraw = true;
                continue;
            }

            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    if contents == self.state.document.text().to_string() {
                        continue;
                    }
                    // The cursor is kept by offset rather than reset: a formatter run should not
                    // send you back to the top of the file.
                    let cursor = self.state.cursor();
                    let previous = self.state.document.version();
                    self.state.document = Document::from_str(&contents);
                    // Versions must keep increasing across the replacement, or the language server
                    // is told to go backwards and every diagnostic after it reads as stale.
                    self.state.document.resume_from(previous);
                    let clamped = cursor.min(self.state.document.text().len_chars());
                    self.state.document.set_primary_caret(clamped);
                    self.after_buffer_change();
                    let name = relative_crumbs(&path.to_string_lossy()).join("/");
                    eprintln!("vex: reloaded {name} — changed on disk");
                    self.state.hover = Some(format!("reloaded {name}"));
                    redraw = true;
                }
                Err(error) => {
                    self.state.hover = Some(format!(
                        "{} changed but could not be read: {error}",
                        path.display()
                    ));
                    redraw = true;
                }
            }
        }
        redraw
    }

    /// Points the tree at the file that just became active.
    fn reveal_in_tree(&mut self) {
        let root = self.index.root().to_path_buf();
        let Some(path) = self.state.file_path.clone() else {
            return;
        };
        let Some(tree) = &mut self.state.tree else {
            return;
        };
        if let Ok(absolute) = std::path::Path::new(&path).canonicalize() {
            if let Ok(relative) = absolute.strip_prefix(&root) {
                tree.reveal(&relative.to_string_lossy());
            }
        }
    }

    /// Resets per-buffer derived state after the active document changes.
    fn after_buffer_change(&mut self) {
        // Pairing rules are per-language, so they travel with the buffer.
        self.state.pairs = pair_config(
            self.state.file_path.as_deref(),
            self.state.display.tab_width,
        );

        // The node path belongs to the old document. Highlights do not need clearing here: they are
        // swapped with the buffer, so what is in `state` already belongs to the document in `state`.
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

        self.reveal_in_tree();
        self.watch_active();

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
        let panes = pane_views(&self.panes, &self.state, &self.buffers);
        chrome::draw_panes(
            &mut self.frame,
            &self.state,
            &panes,
            self.panes.layout,
            Rect::new(0.0, 0.0, width as f32, height as f32),
            scale,
        );
        drop(panes);

        if let Err(err) = renderer.render(&self.frame, self.state.theme.bg) {
            eprintln!("vex: frame dropped: {err:#}");
        }
    }
}

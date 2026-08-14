//! `vex` — the editor binary.
//!
//! Wires the layers together: `vex-text` (storage), `vex-buffer` (document and undo), `vex-display`
//! (soft wrap, folding, viewport), `vex-input` (the grammar), `vex-syntax`, `vex-lsp`, `vex-project`,
//! `vex-lua` and `vex-render`. What lives *here* is the event loop, the window, and the wiring: this
//! file owns the things that cannot be tested without a window, and delegates everything that can.

// A release build on Windows is a GUI application, not a console one. Without this, launching from the
// Start Menu opens a black console window behind the editor, which looks like a fault. Debug builds
// keep the console, because that is where `eprintln!` diagnostics go and losing them while developing
// would be the worse trade.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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
use vex_text::{CharOffset, Selection, SelectionSet, Text, Version};
use vex_theme::{Mode, Theme, space};
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

    let argument = positional(&args);

    // `vex some/dir` opens a *project*, the way `vex file.rs` opens a file. The directory itself
    // becomes the root rather than searching upwards from it for a `.git`: naming a folder is an
    // explicit choice about what the project is, and second-guessing it would index the parent
    // repository when you asked for one crate of it.
    let folder = argument
        .as_ref()
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_dir())
        .map(|path| vex_project::canonical(&path));
    let path = if folder.is_some() { None } else { argument };

    // With no file named — including when a folder was — the last session for that project is what
    // to open. Naming a file explicitly wins: asking for one file and getting the last three back
    // would be surprising.
    let restore = (path.is_none() && !args.iter().any(|arg| arg == "--no-session"))
        .then(|| {
            let anchor = folder
                .clone()
                .unwrap_or_else(|| find_root(&std::env::current_dir().unwrap_or_default()));
            vex::session::load(&vex::session::session_dir(), &anchor)
        })
        .flatten();

    let first = restore
        .as_ref()
        .and_then(|session| session.files.get(session.active))
        .map(|file| file.path.to_string_lossy().to_string());
    let (text, crumbs, file_path) = load(path.or(first))?;

    // The project root anchors file discovery: the folder if one was named, otherwise a marker
    // search from the opened file, so opening a stray file does not index a home directory.
    let root = match &folder {
        Some(folder) => folder.clone(),
        None => find_root(&std::path::PathBuf::from(
            file_path.clone().unwrap_or_else(|| ".".to_string()),
        )),
    };
    eprintln!("vex: project root {}", root.display());

    let event_loop = EventLoop::new().context("creating the event loop")?;
    // Wait, not Poll: an editor redraws in response to input, not continuously. Polling would
    // spin the GPU and the fan for a screen that is not changing.
    event_loop.set_control_flow(ControlFlow::Wait);

    // The mode the editor starts in depends on the setting: with modal editing off there is only
    // one behaviour, and starting in "normal" would swallow the first thing typed.
    let starting_settings = config::load();
    let starting_mode = if starting_settings.modal {
        Mode::Normal
    } else {
        Mode::Insert
    };

    let (state, keymap) = new_state(text, crumbs, file_path, starting_mode, None, root.clone());
    let mut grammar = Grammar::new();
    grammar.set_keymap(keymap);
    grammar.set_modal(starting_settings.modal);

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
        servers: std::collections::HashMap::new(),
        active_server: None,
        sent_version: None,
        folder_picker: None,
        snippets: load_snippets(),
        snippet: None,
        jumps: vex_input::JumpList::default(),
        macros: vex_input::Macros::default(),
        code_actions: Vec::new(),
        hinted_version: None,
        recovery: Recovery::new(),
        git: vex::git::GitWorker::spawn(),
        diffed_version: None,
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
        pointer: (0.0, 0.0),
        dragging: false,
        last_click: None,
        click_count: 1,
        wakes: 0,
        index: FileIndex::new(root.clone()),
        walk: Some(FileWalk::start(root.clone())),
        buffers: Vec::new(),
        state,
    };

    // First run: no configuration means nobody has answered the two questions that decide whether
    // this editor is usable for them at all.
    if vex::welcome::is_first_run(&vex::config::config_dir())
        && !args.iter().any(|arg| arg == "--no-welcome")
    {
        app.state.welcome = Some(vex::welcome::Welcome::new());
    }

    // Opening a folder shows what is in it. Nothing else would answer the question the user just
    // asked by naming a directory.
    if folder.is_some() {
        app.show_tree();
    }

    // The rest of the session's files, opened behind the active one so `ctrl-b` finds them.
    if let Some(session) = restore {
        let active = session
            .files
            .get(session.active)
            .map(|file| file.path.clone());
        let others = session.files.len().saturating_sub(1);

        for file in session.files.iter().rev() {
            if Some(&file.path) == active.as_ref() {
                continue;
            }
            app.open_in_background(&file.path, file.line, file.column);
        }
        // Cursor restoration for the active file, which was opened before the app existed.
        if let Some(file) = session.files.get(session.active) {
            app.restore_cursor(file.line, file.column);
        }
        if others > 0 {
            eprintln!("vex: restored {} more from the last session", others);
        }
    }

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
        modal: settings.modal,
        sticky_scroll: settings.sticky_scroll,
        recording: None,
        diff: vex::git::Diff::default(),
        show_diagnostics: false,
        inlay_hints: Vec::new(),
        show_inlay_hints: settings.inlay_hints,
        welcome: None,
        settings: None,
        setting_sources: settings.sources.clone(),
        tree: None,
        help: None,
        menu: None,
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
const VALUE_FLAGS: [&str; 13] = [
    "--which-key",
    "--screenshot",
    "--mode",
    "--columns",
    "--palette",
    "--search",
    "--grep",
    "--diagnostic",
    "--complete",
    "--split",
    "--at",
    "--theme",
    "--welcome",
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
        value("--theme").and_then(|name| Theme::by_name(&name))
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

    // `--at LINE` parks the cursor on a line, for captures that need a particular part of the file
    // on screen — sticky scroll only appears when a block head has actually scrolled off.
    if flag("--no-vim") {
        state.modal = false;
        state.mode = Mode::Insert;
    }

    if let Some(line) = value("--at").and_then(|v| v.parse::<usize>().ok()) {
        let text = state.document.text();
        let line = line.min(text.last_line());
        let offset = text.point_to_char(vex_text::Point::new(line, 0));
        state.document.set_primary_caret(offset);
        state.scroll = ScrollAnchor::at_line(line.saturating_sub(2));
    }

    // `--diagnostics-list` fills the file with a spread of diagnostics so the sidebar list can be
    // checked. Several, of both severities: a list of one proves nothing about how a list reads.
    if flag("--diagnostics-list") {
        let sample = [
            (
                3u32,
                4u32,
                "mismatched types: expected `u32`, found `&str`",
                true,
            ),
            (11, 8, "unused variable: `offset`", false),
            (24, 0, "cannot find value `bias` in this scope", true),
            (
                37,
                12,
                "unnecessary parentheses around block return value",
                false,
            ),
            (
                52,
                6,
                "this function takes 2 arguments but 3 were supplied",
                true,
            ),
        ];
        let items = sample
            .iter()
            .map(
                |(line, column, message, is_error)| vex_lsp::lsp_types::Diagnostic {
                    range: vex_lsp::lsp_types::Range {
                        start: vex_lsp::lsp_types::Position {
                            line: *line,
                            character: *column,
                        },
                        end: vex_lsp::lsp_types::Position {
                            line: *line,
                            character: column + 10,
                        },
                    },
                    severity: Some(if *is_error {
                        vex_lsp::lsp_types::DiagnosticSeverity::ERROR
                    } else {
                        vex_lsp::lsp_types::DiagnosticSeverity::WARNING
                    }),
                    message: message.to_string(),
                    ..Default::default()
                },
            )
            .collect();
        state
            .diagnostics
            .apply(items, None, state.document.version());
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
    } else if flag("--git") {
        // A canned diff against the file on screen, so the gutter marks are inspectable without
        // depending on what happens to be uncommitted right now.
        state.diff = vex::git::Diff::from_marks(
            vec![
                (12, vex::git::Change::Added),
                (13, vex::git::Change::Added),
                (17, vex::git::Change::Modified),
                (24, vex::git::Change::Removed),
                (28, vex::git::Change::Modified),
            ],
            state.document.version(),
        );
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--folded") {
        // Fold every top-level block, so the marker and the hidden-line count are inspectable.
        let mut line = 0;
        let text = state.document.text();
        while line <= text.last_line() {
            match vex_display::fold_range(text, line, state.display.tab_width) {
                Some(range) => {
                    let end = range.end;
                    state.display.folds.close(range);
                    line = end + 1;
                }
                None => line += 1,
            }
        }
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--diagnostics-list") {
        state.show_diagnostics = true;
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if let Some(step) = value("--welcome") {
        let mut welcome = vex::welcome::Welcome::new();
        // Walk to the requested step, so each screen can be captured on its own.
        if step != "keys" {
            welcome.choose();
        }
        if step == "import" {
            welcome.choose();
        }
        if step == "theme" {
            // Highlight something other than the first, which is what makes the preview visible.
            welcome.move_selection(2);
        }
        chrome::draw_welcome(&mut frame, &welcome, &state.typography, bounds, 1.0);
    } else if flag("--help-panel") {
        state.help = Some(vex::help::Help::new(state.modal));
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--menu") {
        state.menu = Some(vex::menu::Menu::bar());
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--context-menu") {
        state.menu = Some(vex::menu::Menu::context((420.0, 300.0), state.modal));
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if let Some(prefix) = value("--which-key") {
        // The popup is driven by the pending display, exactly as a half-typed chord drives it.
        state.pending = prefix;
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--commands") {
        let mut palette = vex::palette::Palette::new(vex::palette::PaletteKind::Commands);
        palette.refresh_commands(state.modal);
        state.palette = Some(palette);
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--settings") {
        state.settings = Some(vex::settings_ui::Settings::new());
        state
            .setting_sources
            .insert("tab_width".into(), vex::settings_ui::Source::Config);
        state
            .setting_sources
            .insert("soft_wrap".into(), vex::settings_ui::Source::Imported);
        chrome::draw(&mut frame, &state, bounds, 1.0);
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
    /// Every language server running, keyed by program name.
    servers: std::collections::HashMap<String, vex_lsp::LspClient>,
    /// Which one the buffer on screen belongs to.
    active_server: Option<String>,
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
    /// An open folder dialog, if one is up. Polled like every other background service.
    folder_picker: Option<vex::picker::FolderPicker>,
    /// Snippets loaded from the config directory, for `tab` expansion.
    snippets: Vec<vex_import::Snippet>,
    /// The tab stops of the snippet being filled in, if one is.
    snippet: Option<vex::snippet::Session>,
    /// Where the cursor has jumped from, for `ctrl-o` / `ctrl-i`.
    jumps: vex_input::JumpList,
    /// Recorded action sequences, for `q` and `@`.
    macros: vex_input::Macros,
    /// The code actions the server last offered, indexed by the palette.
    code_actions: Vec<vex_lsp::CodeActionItem>,
    /// The version inlay hints were last requested for, so typing does not queue one per keystroke.
    hinted_version: Option<vex_text::Version>,
    /// Persists dirty buffers so a crash loses seconds, not work.
    recovery: Recovery,
    /// Diffs the buffer against `HEAD`, off the UI thread.
    git: vex::git::GitWorker,
    /// The version the last diff was asked for, so typing does not spawn a `git show` per keystroke.
    diffed_version: Option<vex_text::Version>,
    /// Watches open files for changes made outside the editor.
    watcher: Option<vex_project::FileWatcher>,
    /// Paths this editor is about to write, so its own saves are not mistaken for someone else's
    /// edit. Without this, every save would come straight back as "changed on disk".
    self_written: std::collections::HashSet<std::path::PathBuf>,
    /// Where the pointer is, in physical pixels.
    pointer: (f32, f32),
    /// Whether the left button is held, so a move extends the selection.
    dragging: bool,
    /// When and where the last click landed, for recognising a double or triple one.
    ///
    /// The platform does not tell us: `winit` reports presses, and the "is this the second click of
    /// a double?" question is the application's. Both halves are needed — a second click a second
    /// later, or an inch away, is a new click and not a word selection.
    last_click: Option<(std::time::Instant, f32, f32)>,
    /// 1, 2 or 3: caret, word, line. Kept for the whole gesture so a drag after a double-click is
    /// still a word-wise drag.
    click_count: u8,
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

        // The config is watched like any other file, so editing `init.lua` in vex shows the result
        // on save rather than on the next launch.
        let config_file = vex::config::config_dir().join("init.lua");
        if config_file.exists() {
            if let Some(watcher) = &mut self.watcher {
                let _ = watcher.watch(&config_file);
            }
        }

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
        changed |= self.sync_git();
        changed |= self.handle_file_changes();
        changed |= self.poll_folder_picker();
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
                self.save_session();
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

            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x as f32, position.y as f32);
                // Dragging extends the selection, which is how every editor spells "select this".
                if self.dragging {
                    self.click(true);
                    self.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                match (button, state) {
                    (winit::event::MouseButton::Left, ElementState::Pressed) => {
                        self.dragging = true;
                        self.click_count = self.count_click();
                        self.click(false);
                    }
                    (winit::event::MouseButton::Left, ElementState::Released) => {
                        self.dragging = false
                    }
                    // The context menu, which is where a mouse user looks for cut, copy and "go to
                    // definition" before they look for a keyboard shortcut.
                    (winit::event::MouseButton::Right, ElementState::Pressed) => {
                        self.open_context_menu()
                    }
                    _ => {}
                }
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
                    // Quitting is the last chance to save unsaved work, and to remember where we
                    // were.
                    self.persist_recovery(true);
                    self.save_session();
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
            // The function keys, which carry the bindings every editor agrees on: F2 rename, F12
            // definition, F8 next error. Written out rather than computed because `NamedKey` has no
            // ordering to compute from.
            WinitKey::Named(NamedKey::F1) => KeyCode::Function(1),
            WinitKey::Named(NamedKey::F2) => KeyCode::Function(2),
            WinitKey::Named(NamedKey::F3) => KeyCode::Function(3),
            WinitKey::Named(NamedKey::F4) => KeyCode::Function(4),
            WinitKey::Named(NamedKey::F5) => KeyCode::Function(5),
            WinitKey::Named(NamedKey::F6) => KeyCode::Function(6),
            WinitKey::Named(NamedKey::F7) => KeyCode::Function(7),
            WinitKey::Named(NamedKey::F8) => KeyCode::Function(8),
            WinitKey::Named(NamedKey::F9) => KeyCode::Function(9),
            WinitKey::Named(NamedKey::F10) => KeyCode::Function(10),
            WinitKey::Named(NamedKey::F11) => KeyCode::Function(11),
            WinitKey::Named(NamedKey::F12) => KeyCode::Function(12),
            WinitKey::Character(text) => KeyCode::Char(text.chars().next()?),
            _ => return None,
        };

        Some(Key {
            code,
            ctrl: self.modifiers.control_key(),
            alt: self.modifiers.alt_key(),
            shift: self.modifiers.shift_key(),
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

        if self.handle_welcome_key(key) {
            return false;
        }
        // The menu and the help panel own the keyboard while they are open — they are things you are
        // reading or choosing from, and a key that fell through would type into the file behind
        // them. Both return whether the editor should quit, which a menu entry can ask for.
        if self.state.menu.is_some() {
            return self.handle_menu_key(key);
        }
        if self.state.help.is_some() {
            return self.handle_help_key(key);
        }
        if self.handle_settings_key(key) {
            return false;
        }
        if self.handle_tree_key(key) {
            return false;
        }

        // Opening a palette is a Ctrl binding, so it works from any mode. Shift matters here:
        // `ctrl-shift-p` is the *command* palette and must not be eaten by the file one — which it
        // was, because this intercept ran before the grammar and ignored the modifier.
        if key.ctrl && !key.shift {
            match key.code {
                KeyCode::Char('p') => {
                    self.open_palette(PaletteKind::Files);
                    return false;
                }
                KeyCode::Char('b') => {
                    self.open_palette(PaletteKind::Buffers);
                    return false;
                }
                _ => {}
            }
        }
        // `ctrl-f` finds in this file and `ctrl-shift-f` across the project, as everywhere else.
        // They were the same key, and it was the project one — so the most-pressed shortcut in
        // computing opened a background search of every file instead of a search box.
        if key.ctrl && key.code == KeyCode::Char('f') {
            let rows = self.code_rows();
            let action = if key.shift {
                Action::SearchProject
            } else {
                Action::StartSearch { forward: true }
            };
            execute(&mut self.state, &mut self.registers, &action, rows);
            return false;
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

    /// Keys for the menu bar and the context menu, which own the keyboard while open.
    ///
    /// Returns whether the editor should quit — `file ▸ quit` is a menu entry, and a menu that
    /// swallowed it would be a menu with a broken row in it.
    fn handle_menu_key(&mut self, key: Key) -> bool {
        let modal = self.state.modal;

        match key.code {
            KeyCode::Escape => self.state.menu = None,
            KeyCode::Enter => {
                let action = self
                    .state
                    .menu
                    .as_ref()
                    .and_then(|menu| menu.selection().map(|item| item.action.clone()));
                // Closed before running: a command that opens something of its own — the palette, a
                // prompt — must not find the menu still sitting on top of it.
                self.state.menu = None;
                if let Some(action) = action {
                    return self.run(action);
                }
            }
            code => {
                if let Some(menu) = &mut self.state.menu {
                    match code {
                        KeyCode::Up => menu.move_selection(-1),
                        KeyCode::Down => menu.move_selection(1),
                        KeyCode::Left => menu.move_menu(-1, modal),
                        KeyCode::Right => menu.move_menu(1, modal),
                        _ => {}
                    }
                }
            }
        }
        false
    }

    /// Keys for the help panel. Typing filters it, which is the only way a list this long answers a
    /// question rather than presenting one.
    fn handle_help_key(&mut self, key: Key) -> bool {
        let rows = self.code_rows();
        match key.code {
            KeyCode::Escape => self.state.help = None,
            code => {
                if let Some(help) = &mut self.state.help {
                    match code {
                        KeyCode::Up => help.scroll_by(-1, rows),
                        KeyCode::Down => help.scroll_by(1, rows),
                        KeyCode::PageUp => help.scroll_by(-(rows as isize), rows),
                        KeyCode::PageDown => help.scroll_by(rows as isize, rows),
                        KeyCode::Backspace => help.backspace(),
                        KeyCode::Char(c) if !key.ctrl && !key.alt => help.type_char(c),
                        _ => {}
                    }
                }
            }
        }
        false
    }

    /// Opens the context menu where the pointer is.
    fn open_context_menu(&mut self) {
        // The caret moves to the click first: every command in the menu is about the thing under the
        // pointer, and a "go to definition" that used the old cursor position would be a trap.
        if self.state.menu.is_none() {
            self.click(false);
        }
        self.state.menu = Some(vex::menu::Menu::context(self.pointer, self.state.modal));
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
        // Recorded before executing, so a macro captures what was asked for rather than what it
        // happened to do — and before the jumplist, so `ctrl-o` inside a macro replays too.
        self.macros.record(&action);

        // A jump remembers where you were *before* it moves you.
        if vex_input::is_jump(&action) {
            let here = self.here();
            self.jumps.push(here);
        }

        // The unnamed register *is* the system clipboard, so it is filled from outside before a
        // paste and pushed outside after anything that changed it. Done here rather than in
        // `execute`, which must stay free of the machine it runs on to be testable.
        if matches!(action, Action::Paste { register: None, .. }) {
            if let Some(outside) = vex::clipboard::get() {
                self.registers.set(None, outside);
            }
        }
        let before = self.registers.get(None).to_string();

        let outcome = execute(&mut self.state, &mut self.registers, &action, rows);
        if let Outcome::Window(action) = &outcome {
            self.window_action(action);
        }

        // Compared rather than predicted from the action: every path that yanks goes through the
        // register, and a list of which actions those are would drift the first time one was added.
        let after = self.registers.get(None);
        if after != before {
            vex::clipboard::set(after);
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
        self.sent_version = None;
        self.state.diagnostics.clear();

        let Some(path) = self.state.file_path.clone() else {
            self.active_server = None;
            self.state.server_status = "scratch buffer".into();
            return;
        };
        let Some(spec) = vex_lsp::server_for(&path) else {
            self.active_server = None;
            self.state.server_status = "no server for this language".into();
            return;
        };

        // Already running for another buffer of the same language: switching to a TypeScript file
        // and back to Rust must not restart rust-analyzer, which would throw away minutes of
        // indexing every time you glance at another file.
        if self.servers.contains_key(spec.program) {
            self.active_server = Some(spec.program.to_string());
            self.state.server_status = spec.program.to_string();
            return;
        }

        if !vex_lsp::is_installed(spec.program) {
            self.active_server = None;
            self.state.server_status = format!("{} not installed", spec.program);
            return;
        }

        let root = self.index.root().to_path_buf();
        match vex_lsp::LspClient::start(spec.program, spec.args, &root) {
            Ok(client) => {
                eprintln!("vex: starting {} in {}", spec.program, root.display());
                self.state.server_status = format!("{} starting", spec.program);
                self.servers.insert(spec.program.to_string(), client);
                self.active_server = Some(spec.program.to_string());
            }
            Err(error) => {
                eprintln!("vex: {error:#}");
                self.active_server = None;
                self.state.server_status = format!("{} failed to start", spec.program);
            }
        }
    }

    /// The server for the buffer on screen, if there is one.
    ///
    /// Servers are kept per program rather than one at a time, so a project mixing Rust and
    /// TypeScript keeps both alive. Only the active buffer's server is *talked to*: the others hold
    /// their state and their index, costing memory but no work.
    fn active_client(&self) -> Option<&vex_lsp::LspClient> {
        let name = self.active_server.as_ref()?;
        self.servers.get(name)
    }

    /// Sends document changes and applies whatever the server has said.
    ///
    /// The same contract as the syntax worker: send, poll, and refuse anything computed against a
    /// superseded version.
    fn sync_lsp(&mut self) -> bool {
        let Some(name) = self.active_server.clone() else {
            return false;
        };
        let Some(client) = self.servers.get_mut(&name) else {
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

        // Inlay hints are asked for once per document version, and only when they are on. Tying
        // them to the version rather than to a timer means typing does not queue a request per
        // keystroke, and the reply is never for text that has moved.
        if client.state() == vex_lsp::State::Ready
            && self.state.show_inlay_hints
            && self.hinted_version != Some(buffer_version)
        {
            if let Some(path) = self.state.file_path.clone() {
                let text = self.state.document.text();
                let last = text.last_line();
                let range = vex_lsp::lsp_types::Range {
                    start: vex_lsp::lsp_types::Position {
                        line: 0,
                        character: 0,
                    },
                    end: vex_lsp::lsp_types::Position {
                        line: last as u32,
                        character: 0,
                    },
                };
                if client
                    .inlay_hints(std::path::Path::new(&path), range)
                    .is_ok()
                {
                    self.hinted_version = Some(buffer_version);
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
                        Action::CodeAction => {
                            self.edit_version = Some(self.state.document.version());
                            let range = vex_lsp::lsp_types::Range {
                                start: position,
                                end: position,
                            };
                            // The diagnostics on the cursor's line are the context: without them a
                            // server has no reason to offer the fix for the error you are sitting on.
                            let here: Vec<_> = self
                                .state
                                .diagnostics
                                .items()
                                .iter()
                                .filter(|d| {
                                    d.range.start.line <= position.line
                                        && position.line <= d.range.end.line
                                })
                                .cloned()
                                .collect();
                            client.code_action(&path, range, &here)
                        }
                        Action::FindReferences => client.references(&path, position),
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
            .map(|path| vex_project::canonical(std::path::Path::new(path)));

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
                vex_lsp::ServerEvent::CodeActions { actions } => {
                    if actions.is_empty() {
                        self.state.hover = Some("no code actions here".into());
                    } else {
                        let rows: Vec<(String, String)> = actions
                            .iter()
                            .map(|action| (action.title.clone(), action.kind.clone()))
                            .collect();
                        self.code_actions = actions;
                        // The palette, not a menu of its own: this is a list you filter and choose
                        // from, which is exactly what the palette already is.
                        let mut palette = Palette::new(PaletteKind::CodeActions);
                        palette.refresh_code_actions(&rows);
                        self.state.palette = Some(palette);
                    }
                }
                vex_lsp::ServerEvent::References { locations } => {
                    // Into the results panel rather than a list of its own: references are
                    // `path:line` entries you move through and open, which that panel already is.
                    let hits: Vec<vex_project::SearchHit> = locations
                        .iter()
                        .filter_map(|location| {
                            let path = vex_lsp::uri_to_path(location.uri.as_str())?;
                            let line = std::fs::read_to_string(&path)
                                .ok()
                                .and_then(|text| {
                                    text.lines()
                                        .nth(location.range.start.line as usize)
                                        .map(str::to_string)
                                })
                                .unwrap_or_default();
                            Some(vex_project::SearchHit {
                                path,
                                line_number: location.range.start.line as u64 + 1,
                                line: line.trim_end().to_string(),
                                range: 0..0,
                            })
                        })
                        .collect();

                    if hits.is_empty() {
                        self.state.hover = Some("no references found".into());
                    } else {
                        self.state.results = Some(vex::results::Results::from_hits(
                            &self.state.project_root,
                            "references",
                            hits,
                        ));
                    }
                }
                vex_lsp::ServerEvent::InlayHints { hints } => {
                    self.state.inlay_hints = hints;
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
                    // Dying *before* the handshake means it never was a server: a rustup shim without
                    // the component installed does exactly this. Worth distinguishing, because
                    // "stopped" reads as "it was working and quit".
                    self.state.server_status = if self.sent_version.is_none() {
                        format!("{name} could not start — is it really installed?")
                    } else {
                        "server stopped".into()
                    };
                    self.servers.remove(&name);
                    self.active_server = None;
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
        if let Some(client) = self.active_client() {
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
            .map(vex_project::canonical)
            == Some(vex_project::canonical(&path));

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

        // A live snippet's tab stops move with the text typed into them. Done here because this is
        // where the edit queue is drained — the stops and the syntax tree need the same edits, in
        // the same coordinates, and taking them twice is not possible.
        if let Some(session) = &mut self.snippet {
            for change in &changes {
                session.transform(&change.edits);
            }
            if session.is_finished() {
                // Past the last stop there is nothing left to walk, and holding the session open
                // would make the next `tab` do nothing instead of indenting.
                self.snippet = None;
            }
        }

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
                    PromptKind::OpenFolder => {
                        let raw = prompt.input.trim().to_string();
                        // `~` is expanded here because a prompt is not a shell and nobody types
                        // `$HOME` into one.
                        let expanded = match raw.strip_prefix("~/") {
                            Some(rest) => std::env::var_os("HOME")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_default()
                                .join(rest),
                            None => std::path::PathBuf::from(&raw),
                        };
                        if expanded.is_dir() {
                            self.open_folder(&expanded);
                        } else {
                            self.warn(&format!("not a folder: {}", expanded.display()));
                        }
                    }
                    PromptKind::LoadTheme => {
                        let raw = prompt.input.trim().to_string();
                        self.load_theme_file(&raw);
                    }
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

/// Snippets from the config directory, as the importer writes them.
///
/// Read once at startup. A snippet file that fails to parse is skipped with a note rather than
/// stopping the editor — the same rule the rest of configuration follows.
fn load_snippets() -> Vec<vex_import::Snippet> {
    let directory = vex::config::config_dir().join("snippets");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };

    let mut snippets = Vec::new();
    let mut report = vex_import::ImportReport::default();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| vex_import::parse_jsonc(&text).ok())
        {
            Some(json) => snippets.extend(vex_import::import_snippets(&json, &mut report)),
            None => eprintln!("vex: could not read snippets from {}", path.display()),
        }
    }

    if !snippets.is_empty() {
        eprintln!(
            "vex: {} snippet{} loaded",
            snippets.len(),
            if snippets.len() == 1 { "" } else { "s" }
        );
    }
    snippets
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
            Action::ExpandSnippet => self.expand_snippet(),
            // The three ways of finding out what the editor can do. Each is a toggle, because the
            // key that opened it is the key someone presses again to get rid of it.
            Action::ToggleCommandPalette => {
                if self.state.palette.is_some() {
                    self.state.palette = None;
                } else {
                    self.open_palette(PaletteKind::Commands);
                }
            }
            Action::ToggleHelp => {
                self.state.help = match self.state.help {
                    Some(_) => None,
                    None => Some(vex::help::Help::new(self.state.modal)),
                };
            }
            Action::ToggleMenu => {
                self.state.menu = match self.state.menu {
                    Some(_) => None,
                    None => Some(vex::menu::Menu::bar()),
                };
            }
            Action::ToggleContextMenu => self.open_context_menu(),
            Action::OpenFolder => {
                // The desktop's own folder chooser when there is one — that is what "open a folder"
                // means to everyone who did not come from Vim. Typing a path is the fallback, not the
                // interface.
                if vex::picker::is_available() {
                    if self.folder_picker.is_none() {
                        let start = self
                            .state
                            .project_root
                            .parent()
                            .unwrap_or(&self.state.project_root)
                            .to_path_buf();
                        self.folder_picker = Some(vex::picker::FolderPicker::open(start));
                        self.state.hover = Some("choose a folder…".into());
                    }
                } else {
                    let mut prompt =
                        vex::prompt::Prompt::new(PromptKind::OpenFolder, self.state.cursor());
                    // Pre-filled with the current root and a separator, so the common case — a
                    // sibling project — is a few characters rather than a whole path.
                    prompt.input = format!("{}/", self.state.project_root.display());
                    self.state.prompt = Some(prompt);
                }
            }
            Action::ToggleSettings => {
                if self.state.settings.is_some() {
                    self.state.settings = None;
                } else {
                    self.state.settings = Some(vex::settings_ui::Settings::new());
                    self.state.tree = None;
                    self.state.show_diagnostics = false;
                }
            }
            Action::Jump { forward } => {
                let here = self.here();
                let target = if *forward {
                    self.jumps.forward()
                } else {
                    self.jumps.back(here)
                };
                match target {
                    Some(jump) => self.go_to(jump),
                    None => {
                        self.state.hover = Some(
                            if *forward {
                                "no newer position in the jumplist"
                            } else {
                                "no older position in the jumplist"
                            }
                            .into(),
                        )
                    }
                }
            }
            Action::RecordMacro { register } => {
                if self.macros.toggle(*register) {
                    self.state.hover = Some(format!("recorded @{register}"));
                }
                self.state.recording = self.macros.recording_register();
            }
            Action::ReplayMacro { register } => {
                let Some(actions) = self.macros.replay(*register) else {
                    self.state.hover = Some("that register is empty".into());
                    return;
                };
                let rows = self.code_rows();
                // Replayed as one undo group: a macro is one intent, however many edits it makes,
                // and undoing it a keystroke at a time is what makes macros frightening elsewhere.
                self.state.document.end_undo_group();
                for action in actions {
                    if let Outcome::Quit =
                        execute(&mut self.state, &mut self.registers, &action, rows)
                    {
                        break;
                    }
                }
                self.state.document.end_undo_group();
                self.reveal_cursor();
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
                        let absolute = vex_project::canonical(std::path::Path::new(path));
                        if let Ok(relative) = absolute.strip_prefix(self.index.root()) {
                            tree.reveal(&relative.to_string_lossy());
                        }
                    }
                    self.state.tree = Some(tree);
                    self.state.show_diagnostics = false;
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

    /// Opens a file without making it active — for restoring a session's other buffers.
    fn open_in_background(&mut self, path: &std::path::Path, line: usize, column: usize) {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return;
        };
        let mut document = Document::from_str(&contents);
        let offset = document
            .text()
            .point_to_char(vex_text::Point::new(line, column));
        document.set_primary_caret(offset);

        let display = path.to_string_lossy().to_string();
        self.buffers.push(Buffer {
            document,
            path: relative_crumbs(&display),
            file_path: Some(display),
            scroll: ScrollAnchor::default(),
            highlights: Vec::new(),
        });
        self.state.buffer_count = self.buffers.len() + 1;
    }

    /// Puts the cursor back where the session left it.
    fn restore_cursor(&mut self, line: usize, column: usize) {
        let text = self.state.document.text();
        let line = line.min(text.last_line());
        let offset = text.point_to_char(vex_text::Point::new(line, column));
        self.state.document.set_primary_caret(offset);
    }

    /// Records the open files and where the cursor is in each, for the next run.
    ///
    /// Written on the way out rather than continuously: unlike crash recovery, which must survive a
    /// kill, a session is only interesting when the editor closed properly.
    fn save_session(&mut self) {
        self.save_pane();

        let mut files = Vec::new();
        let mut active = 0;

        for index in 0..=self.buffers.len() {
            let (path, offset, text) = if index == self.state.active_buffer {
                active = files.len();
                (
                    self.state.file_path.clone(),
                    self.state.cursor(),
                    self.state.document.text(),
                )
            } else {
                match stored_index(index, self.state.active_buffer)
                    .and_then(|stored| self.buffers.get(stored))
                {
                    Some(buffer) => (
                        buffer.file_path.clone(),
                        buffer.document.selections().primary().head,
                        buffer.document.text(),
                    ),
                    None => continue,
                }
            };

            // Only files with a path: a scratch buffer has nothing to reopen, and crash recovery is
            // what protects its contents.
            let Some(path) = path else { continue };
            let point = text.char_to_point(offset);
            files.push(vex::session::OpenFile {
                path: std::path::PathBuf::from(path),
                line: point.line,
                column: point.column,
            });
        }

        let session = vex::session::Session { files, active };
        vex::session::save(&vex::session::session_dir(), self.index.root(), &session);
    }

    /// `tab` in insert mode: move to the next stop, expand a snippet, or indent.
    ///
    /// In that order, because each is what the key should mean when the one before it does not
    /// apply — and because `tab` has to keep being the indent key for everyone with no snippets.
    fn expand_snippet(&mut self) {
        // Tab over a selection that covers more than one line indents the block, which is what tab
        // means in every editor that is not Vim. Checked before the snippet lookup because there is
        // no snippet in play when you have deliberately selected six lines — and because the
        // alternative, replacing the selection with four spaces, destroys work.
        let selection = *self.state.document.selections().primary();
        let text = self.state.document.text();
        if text.char_to_line(selection.start()) != text.char_to_line(selection.end()) {
            let rows = self.code_rows();
            execute(
                &mut self.state,
                &mut self.registers,
                &Action::Apply {
                    operator: vex_input::Operator::Indent,
                    target: vex_input::Target::Selection,
                    register: None,
                },
                rows,
            );
            return;
        }

        // Already filling one in: move to the next stop.
        if let Some(session) = &mut self.snippet {
            if session.advance() {
                if let Some((at, length)) = session.current() {
                    let selection = if length == 0 {
                        Selection::caret(at)
                    } else {
                        Selection::new(at, CharOffset(at.get() + length))
                    };
                    self.state
                        .document
                        .set_selections(SelectionSet::single(selection));
                    self.reveal_cursor();
                }
                return;
            }
            self.snippet = None;
        }

        let cursor = self.state.cursor();
        let text = self.state.document.text();
        let line_start =
            text.point_to_char(vex_text::Point::new(text.char_to_point(cursor).line, 0));
        let before = text.slice_to_string(line_start..cursor);

        let Some(snippet) = vex::snippet::trigger(&before, &self.snippets).cloned() else {
            self.state.document.insert("    ");
            return;
        };

        let indent: String = before.chars().take_while(|c| c.is_whitespace()).collect();
        let expansion = vex::snippet::expand(&snippet.body, &indent);

        // The prefix the user typed is replaced by the expansion, as one edit — so `tab` undoes in
        // one press rather than leaving half a snippet behind.
        let prefix_len = snippet.prefix.chars().count();
        let start = CharOffset(cursor.get().saturating_sub(prefix_len));
        let transaction = vex_text::Transaction::new(vec![vex_text::Edit::replace(
            start..cursor,
            expansion.text.clone(),
        )]);
        if self
            .state
            .document
            .apply(transaction, vex_buffer::EditKind::Discrete)
            .is_err()
        {
            return;
        }

        let session = vex::snippet::Session::new(&expansion, start);
        let first = session
            .as_ref()
            .and_then(|session| session.current())
            .unwrap_or((
                CharOffset(start.get() + expansion.stops.last().copied().unwrap_or(0)),
                0,
            ));

        let selection = if first.1 == 0 {
            Selection::caret(first.0)
        } else {
            Selection::new(first.0, CharOffset(first.0.get() + first.1))
        };
        self.state
            .document
            .set_selections(SelectionSet::single(selection));
        self.snippet = session;
        self.reveal_cursor();
    }

    /// Where the cursor is, as a jumplist entry.
    fn here(&self) -> vex_input::Jump {
        let point = self.state.cursor_point();
        vex_input::Jump {
            path: self.state.file_path.clone(),
            line: point.line,
            column: point.column,
        }
    }

    /// Goes to a remembered position, opening its file if it is not the one on screen.
    fn go_to(&mut self, jump: vex_input::Jump) {
        if jump.path != self.state.file_path {
            match &jump.path {
                Some(path) => self.activate(Target::File(std::path::PathBuf::from(path))),
                // A scratch buffer cannot be reopened by path, so the jump is dropped rather than
                // landing in whatever file happens to be open.
                None => return,
            }
        }

        let text = self.state.document.text();
        let line = jump.line.min(text.last_line());
        let offset = text.point_to_char(vex_text::Point::new(line, jump.column));
        self.state.document.set_primary_caret(offset);
        self.reveal_cursor();
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
        let ready = self
            .active_client()
            .map(|client| client.state() == vex_lsp::State::Ready);
        match ready {
            None => {
                self.warn("rename needs a language server");
                return;
            }
            Some(false) => {
                self.warn(&format!("{} is still starting", self.state.server_status));
                return;
            }
            Some(true) => {}
        }

        let position = vex_lsp::to_lsp(self.state.document.text(), self.state.cursor());
        self.edit_version = Some(self.state.document.version());

        let sent = self
            .active_client()
            .map(|client| client.rename(std::path::Path::new(&path), position, new_name));
        if let Some(Err(error)) = sent {
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

    /// Keys for the first-run setup. Returns true if the key was consumed — which is always, while it
    /// is showing: it is a modal screen, and letting keys through to a document nobody can see would
    /// be worse than ignoring them.
    fn handle_welcome_key(&mut self, key: Key) -> bool {
        let Some(welcome) = &mut self.state.welcome else {
            return false;
        };

        match key.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Right => welcome.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Left
                if key.code == KeyCode::Left && welcome.step != vex::welcome::Step::Theme =>
            {
                // Left goes back a step everywhere except the theme row, where it moves between cards.
                if !welcome.back() {
                    welcome.move_selection(-1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Left => welcome.move_selection(-1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                welcome.choose();
                if welcome.is_done() {
                    self.finish_welcome();
                }
            }
            KeyCode::Backspace => {
                welcome.back();
            }
            // Skipping leaves no config behind, so the wizard returns next time rather than silently
            // never appearing again after being dismissed by accident.
            KeyCode::Escape => {
                self.state.welcome = None;
                self.state.hover = Some("setup skipped — ctrl-, opens the settings".into());
            }
            _ => {}
        }
        true
    }

    /// Applies the wizard's answers, writes them, and runs any import that was asked for.
    fn finish_welcome(&mut self) {
        let Some(welcome) = self.state.welcome.take() else {
            return;
        };
        let answers = welcome.answers.clone();

        // Applied first, so the editor is already the way they asked before anything else happens.
        self.state.modal = answers.modal;
        self.grammar.set_modal(answers.modal);
        if !answers.modal {
            self.state.mode = Mode::Insert;
        }
        if let Some(theme) = Theme::by_name(answers.theme) {
            self.state.theme = theme;
        }

        let directory = vex::config::config_dir();
        let path = directory.join("init.lua");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let written = vex::welcome::config_from(&answers, &existing);

        if std::fs::create_dir_all(&directory).is_ok() {
            self.self_written.insert(vex_project::canonical(&path));
            match std::fs::write(&path, written) {
                Ok(()) => {
                    self.self_written.insert(vex_project::canonical(&path));
                    eprintln!("vex: wrote {}", path.display());
                }
                Err(error) => {
                    self.state.hover = Some(format!("could not write {}: {error}", path.display()))
                }
            }
        }

        // The import last: it rewrites configuration too, and doing it after means the reload picks
        // up both at once.
        if !answers.import.is_empty() {
            let checklist = answers
                .import
                .iter()
                .fold(vex_import::Checklist::nothing(), |list, item| {
                    list.with(item.item())
                });
            match vex::import_cli::import_from_installed_vscode(checklist, &directory) {
                Ok(report) => {
                    let rendered = report.render();
                    eprintln!("{rendered}");
                    self.reload_config();
                    self.state.hover = Some(rendered);
                }
                Err(error) => self.state.hover = Some(format!("import failed: {error}")),
            }
        }

        self.state
            .setting_sources
            .insert("vim_keys".into(), vex::settings_ui::Source::Config);
        self.state
            .setting_sources
            .insert("theme".into(), vex::settings_ui::Source::Config);
    }

    /// Keys for the settings panel. Returns true if the key was consumed.
    fn handle_settings_key(&mut self, key: Key) -> bool {
        let Some(settings) = &mut self.state.settings else {
            return false;
        };
        if key.ctrl || key.alt {
            return false;
        }

        let line = settings.line();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => settings.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => settings.move_selection(-1),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => match line
            {
                vex::settings_ui::Line::Option(field) => {
                    settings.dirty = true;
                    self.adjust_setting(field, 1);
                }
                vex::settings_ui::Line::Import(item) => settings.tick(item),
                vex::settings_ui::Line::RunImport => self.run_import(),
                vex::settings_ui::Line::LoadTheme => self.prompt_for_theme_file(),
            },
            KeyCode::Char('h') | KeyCode::Left => match line {
                vex::settings_ui::Line::Option(field) => {
                    settings.dirty = true;
                    self.adjust_setting(field, -1);
                }
                vex::settings_ui::Line::Import(item) => settings.tick(item),
                vex::settings_ui::Line::RunImport | vex::settings_ui::Line::LoadTheme => {}
            },
            // `w` writes; nothing is written without asking, because config is a file the user owns.
            KeyCode::Char('w') => self.write_settings(),
            KeyCode::Escape => self.state.settings = None,
            _ => return false,
        }
        true
    }

    /// The panel's current values, read from the live editor rather than from a copy.
    fn setting_values(&self) -> vex::settings_ui::Values {
        vex::settings_ui::Values {
            font_size: self.state.typography.code_size,
            tab_width: self.state.display.tab_width,
            soft_wrap: self.state.display.mode == WrapMode::Word,
            format_on_save: self.state.format_on_save,
            modal: self.state.modal,
            theme: self.state.theme.name(),
            sticky_scroll: self.state.sticky_scroll,
            inlay_hints: self.state.show_inlay_hints,
        }
    }

    /// Applies a change immediately. A settings panel whose effect you cannot see is a form.
    fn adjust_setting(&mut self, field: vex::settings_ui::Field, delta: i32) {
        use vex::settings_ui::Field;

        let mut values = self.setting_values();
        values.adjust(field, delta);

        match field {
            Field::VimKeys => {
                self.state.modal = values.modal;
                self.grammar.set_modal(values.modal);
                // Modes only exist with the grammar on; leaving the editor in "visual" with no way to
                // leave it would be a trap.
                if !values.modal {
                    self.state.mode = Mode::Insert;
                }
            }
            Field::FontSize => {
                self.state.typography = vex_theme::Typography::with_code_size(values.font_size);
                if let Some(window) = &self.window {
                    let scale = window.scale_factor() as f32;
                    let width = window.inner_size().width as f32;
                    fit_wrap_columns(&mut self.state, width, scale);
                }
            }
            Field::TabWidth => {
                self.state.display.tab_width = values.tab_width;
                self.state.pairs.tab_width = values.tab_width;
            }
            Field::SoftWrap => {
                self.state.display.mode = if values.soft_wrap {
                    WrapMode::Word
                } else {
                    WrapMode::None
                };
            }
            Field::FormatOnSave => self.state.format_on_save = values.format_on_save,
            Field::Theme => {
                // Only the built-ins: a theme loaded from a file has colours the panel has no
                // business inventing a variant of, and `Values` reports that case honestly.
                if let Some(theme) = values.theme.and_then(Theme::by_name) {
                    self.state.theme = theme;
                }
            }
            Field::StickyScroll => self.state.sticky_scroll = values.sticky_scroll,
            Field::InlayHints => {
                self.state.show_inlay_hints = values.inlay_hints;
                self.state.inlay_hints.clear();
                self.hinted_version = None;
            }
        }
    }

    /// Asks for a colour-scheme file.
    fn prompt_for_theme_file(&mut self) {
        let mut prompt = vex::prompt::Prompt::new(PromptKind::LoadTheme, self.state.cursor());
        // Pre-filled with the home directory, since a downloaded theme is usually in Downloads.
        prompt.input = format!("{}/", vex::paths::home().display());
        self.state.prompt = Some(prompt);
    }

    /// Reads a colour scheme in any supported format, applies it, and keeps it.
    ///
    /// Converted to vex's own TOML and stored under `themes/`, so it survives a restart and can be
    /// named in `init.lua` — a theme that only lasted for one session would be a demo, not an import.
    fn load_theme_file(&mut self, raw: &str) {
        let expanded = match raw.strip_prefix("~/") {
            Some(rest) => vex::paths::home().join(rest),
            None => std::path::PathBuf::from(raw),
        };

        let Ok(text) = std::fs::read_to_string(&expanded) else {
            self.warn(&format!("could not read {}", expanded.display()));
            return;
        };
        let name = expanded
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut report = vex_import::ImportReport::default();
        let Some((theme, recognised)) = vex_import::read_palette(&name, &text, &mut report) else {
            self.warn(&format!(
                "not a colour scheme vex can read ({}): {}",
                vex_import::Format::EXTENSIONS,
                report.notes().first().cloned().unwrap_or_default()
            ));
            return;
        };

        self.state.theme = theme;

        // Kept as vex's own format, under a slug of the file's name.
        let slug: String = name
            .trim_end_matches(".json")
            .trim_end_matches(".tmTheme")
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml")
            .trim_end_matches(".toml")
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let slug = slug.trim_matches('-').to_string();

        let directory = vex::config::config_dir().join("themes");
        let path = directory.join(format!("{slug}.toml"));
        if std::fs::create_dir_all(&directory).is_ok()
            && std::fs::write(&path, vex::import_cli::render_theme(&slug, &theme)).is_ok()
        {
            self.state.hover = Some(format!(
                "read {recognised} colours from {name} — kept as `{slug}`, so `theme = \"{slug}\"` \
                 in init.lua makes it stick"
            ));
        } else {
            self.state.hover = Some(format!(
                "read {recognised} colours from {name}, but could not save it to {}",
                path.display()
            ));
        }
    }

    /// Runs the ticked import, then reloads configuration so the result is visible at once.
    fn run_import(&mut self) {
        let Some(settings) = &self.state.settings else {
            return;
        };
        let checklist = settings.checklist();
        if checklist.is_empty() {
            self.state.hover =
                Some("tick what to import first — nothing is imported unless you ask".into());
            return;
        }

        let directory = vex::config::config_dir();
        match vex::import_cli::import_from_installed_vscode(checklist, &directory) {
            Ok(report) => {
                let rendered = report.render();
                eprintln!("{rendered}");

                // The imported files are only config; reloading is what makes them take effect, and
                // doing it here means the panel's own values update too.
                self.reload_config();

                let summary = rendered
                    .lines()
                    .find(|line| line.contains("imported") || line.contains("nothing"))
                    .unwrap_or("import finished")
                    .trim()
                    .to_string();
                if let Some(settings) = &mut self.state.settings {
                    settings.import_report = Some(summary.clone());
                }
                self.state.hover = Some(rendered);
            }
            Err(error) => {
                let message = format!("import failed: {error}");
                if let Some(settings) = &mut self.state.settings {
                    settings.import_report = Some(message.clone());
                }
                self.state.hover = Some(message);
            }
        }
    }

    /// Writes the panel's values into `init.lua`.
    fn write_settings(&mut self) {
        let directory = vex::config::config_dir();
        let path = directory.join("init.lua");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = vex::settings_ui::write_back(&existing, &self.setting_values());

        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.state.hover = Some(format!("could not create {}: {error}", directory.display()));
            return;
        }

        // The editor's own write, so the watcher does not treat it as an outside edit and reload.
        self.self_written.insert(vex_project::canonical(&path));
        match std::fs::write(&path, updated) {
            Ok(()) => {
                self.self_written.insert(vex_project::canonical(&path));
                if let Some(settings) = &mut self.state.settings {
                    settings.dirty = false;
                }
                self.state.hover = Some(format!("written to {}", path.display()));
            }
            Err(error) => {
                self.state.hover = Some(format!("could not write {}: {error}", path.display()))
            }
        }
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
                self.open_hit();
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

        self.self_written
            .insert(vex_project::canonical(std::path::Path::new(&path)));

        match std::fs::write(&path, &text) {
            Ok(()) => {
                // Canonicalization only works once the file exists, so a first save records it here.
                self.self_written
                    .insert(vex_project::canonical(std::path::Path::new(&path)));
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
                        .active_client()
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
            PaletteKind::Commands => palette.refresh_commands(self.state.modal),
            PaletteKind::CodeActions => {}
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
        let modal = self.state.modal;
        if let Some(palette) = &mut self.state.palette {
            match palette.kind {
                PaletteKind::Files => palette.refresh_files(&mut self.index),
                PaletteKind::Buffers => palette.refresh_buffers(&names),
                PaletteKind::Commands => palette.refresh_commands(modal),
                // Filtered against the list already fetched: the server is asked once per
                // invocation, and typing narrows what it offered.
                PaletteKind::CodeActions => {
                    let rows: Vec<(String, String)> = self
                        .code_actions
                        .iter()
                        .map(|action| (action.title.clone(), action.kind.clone()))
                        .collect();
                    palette.refresh_code_actions(&rows);
                }
            }
        }
    }

    /// Opens a file or switches to an already-open buffer.
    fn activate(&mut self, target: Target) {
        match target {
            Target::File(path) if path.is_dir() => self.open_folder(&path),
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
            Target::CodeAction(index) => self.apply_code_action(index),
            // Chosen from the palette, run exactly as if its key had been pressed — same action,
            // same executor, same macro recording. A palette that had its own path to each command
            // would be a second implementation of every one of them.
            Target::Command(name) => {
                if let Some(action) = Action::from_name(name) {
                    let _ = self.run(action);
                }
            }
        }
    }

    /// Applies the code action the user picked.
    fn apply_code_action(&mut self, index: usize) {
        let Some(action) = self.code_actions.get(index).cloned() else {
            return;
        };
        let title = action.title.clone();
        self.apply_server_edit(&title, move |state, version| {
            let path = state.file_path.clone().map(std::path::PathBuf::from);
            vex::edits::apply_workspace_edit(
                &mut state.document,
                path.as_deref(),
                &action.edit,
                version,
            )
        });
        self.code_actions.clear();
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

    /// Keeps the git gutter in step with the buffer.
    ///
    /// Version-tagged like every other background result: a diff computed for text that has since
    /// changed is discarded rather than drawn, so the marks never describe a document that is no
    /// longer on screen.
    fn sync_git(&mut self) -> bool {
        let version = self.state.document.version();

        if self.diffed_version != Some(version) {
            if let Some(path) = &self.state.file_path {
                self.git.request(vex::git::DiffRequest {
                    path: std::path::PathBuf::from(path),
                    text: self.state.document.text().to_string(),
                    version,
                });
                self.diffed_version = Some(version);
            }
        }

        match self.git.poll() {
            Some(diff) if diff.version() == Some(version) => {
                self.state.diff = diff;
                true
            }
            // Stale: the buffer moved on while the diff ran. Dropped, and the newer request that
            // caused the move is already queued.
            Some(_) => false,
            None => false,
        }
    }

    /// Opens the selected search hit, at its line. Shared by the keyboard and the mouse.
    fn open_hit(&mut self) {
        let chosen = self
            .state
            .results
            .as_ref()
            .and_then(|results| results.selection())
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
    }

    /// The chrome's geometry for the current window.
    fn layout(&self) -> vex::layout::Layout {
        let (width, height) = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.size())
            .unwrap_or((1280, 820));
        vex::layout::Layout::of(
            Rect::new(0.0, 0.0, width as f32, height as f32),
            self.scale(),
            self.state
                .results
                .as_ref()
                .map(|results| results.visible().0.len()),
        )
    }

    /// Handles a click, or a drag when `extend` is set.
    ///
    /// Reads the same geometry the chrome draws from, so a click lands where it looks like it should
    /// even in a three-way split.
    fn click(&mut self, extend: bool) {
        let (x, y) = self.pointer;
        let scale = self.scale();
        let layout = self.layout();

        // An open menu takes the click, wherever it lands: on a row it runs it, on a title it opens
        // that menu, anywhere else it closes — which is what clicking away from a menu means
        // everywhere, and the only way out for someone who opened one by accident.
        if self.state.menu.is_some() && !extend {
            self.click_menu(x, y, scale);
            return;
        }

        // The sidebar owns its own clicks: choosing a file in the tree is the whole reason someone
        // reaches for the mouse there.
        if vex::layout::contains(layout.sidebar, x, y) && !extend {
            self.click_sidebar(layout.sidebar, y, scale);
            return;
        }

        if let Some(panel) = layout.results {
            if vex::layout::contains(panel, x, y) && !extend {
                self.click_results(panel, y, scale);
                return;
            }
        }

        let count = self.panes.len();
        let vertical = self.panes.layout == chrome::PaneLayout::Vertical;
        let Some(index) = layout.pane_at(x, y, count, vertical, scale) else {
            return;
        };

        // Clicking another pane focuses it first, so the click lands in the buffer it looks like it
        // landed in rather than in whichever pane had focus.
        if index != self.panes.active() {
            self.save_pane();
            self.panes.follow(self.state.active_buffer);
            let target = index;
            self.panes.cycle(false);
            while self.panes.active() != target {
                self.panes.cycle(true);
            }
            self.restore_pane();
        }

        let rects = layout.pane(index, count, vertical, scale);
        let row_height = self.state.typography.row_height() * scale;
        let advance = self.state.typography.code_size * 0.6 * scale;

        // Rows and columns from the position, clamped: a click below the last line means the last
        // line, not nothing.
        let row = ((y - rects.code.y) / row_height).floor().max(0.0) as usize;
        let column = (((x - rects.code.x - 8.0 * scale) / advance).round()).max(0.0) as usize;

        let height = self
            .state
            .visible_rows(rects.code.height, row_height)
            .max(1);
        let offset = self.state.display.offset_at_display_point(
            self.state.document.text(),
            self.state.scroll,
            height,
            vex_text::DisplayPoint { row, column },
        );

        // A drag, or shift-click, keeps the anchor and moves the head — the same gesture, and
        // shift-click is the one every editor offers to people who do not like dragging.
        if extend || self.modifiers.shift_key() {
            let anchor = self.state.document.selections().primary().anchor;
            self.state
                .document
                .set_selections(vex_text::SelectionSet::single(Selection::new(
                    anchor, offset,
                )));
            self.sync_syntax();
            return;
        }

        // Alt-click adds a cursor rather than replacing the set. This is how multi-cursor editing is
        // *discovered*: nobody reads a manual to find ctrl-alt-↓, but everybody tries alt-click.
        if self.modifiers.alt_key() {
            let mut selections: Vec<Selection> =
                self.state.document.selections().iter().copied().collect();
            // Alt-clicking an existing cursor removes it, which is how you undo the one you put in
            // the wrong place without starting the whole set again.
            match selections
                .iter()
                .position(|selection| selection.is_caret() && selection.head == offset)
            {
                Some(at) if selections.len() > 1 => {
                    selections.remove(at);
                }
                _ => selections.push(Selection::caret(offset)),
            }
            let primary = selections.len() - 1;
            self.state
                .document
                .set_selections(vex_text::SelectionSet::from_iter(selections, primary));
            self.sync_syntax();
            return;
        }

        self.state.document.set_primary_caret(offset);

        // Ctrl-click is "go to definition", as it is in every editor with a language server. The
        // caret moves first, because the request is about where the caret is.
        if self.modifiers.control_key() {
            let _ = self.run(Action::GotoDefinition);
            return;
        }

        match self.click_count {
            // Double-click takes the word; triple takes the line. Their absence was the most
            // obviously broken-feeling thing about using vex with a mouse. Both go through the
            // action they share with the keyboard rather than reaching into the selection set here,
            // so the mouse cannot drift from what `ctrl-d` and `ctrl-l` do.
            2 => {
                let _ = self.run(Action::SelectNextOccurrence { all: false });
            }
            count if count >= 3 => {
                let _ = self.run(Action::SelectLine { count: 1 });
            }
            _ => {}
        }

        self.sync_syntax();
    }

    /// A click while a menu is open.
    fn click_menu(&mut self, x: f32, y: f32, scale: f32) {
        let bounds = {
            let (width, height) = self
                .renderer
                .as_ref()
                .map(|renderer| renderer.size())
                .unwrap_or((1280, 820));
            Rect::new(0.0, 0.0, width as f32, height as f32)
        };
        let modal = self.state.modal;

        let Some(menu) = &mut self.state.menu else {
            return;
        };
        let geometry = menu.geometry(bounds, scale);

        if let Some(index) = menu.title_at(&geometry, x, y) {
            menu.open = index;
            menu.selected = 0;
            menu.fill(modal);
            return;
        }

        let chosen = menu
            .row_at(&geometry, x, y)
            .and_then(|index| menu.items.get(index))
            .map(|item| item.action.clone());

        self.state.menu = None;
        if let Some(action) = chosen {
            let _ = self.run(action);
        }
    }

    /// Which click of a multi-click this is.
    ///
    /// The window system does not answer this, so the rule is the usual one: within half a second
    /// and within a few pixels of the last press. The distance test matters as much as the time —
    /// two deliberate clicks in different places are two clicks however fast you are.
    fn count_click(&mut self) -> u8 {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
        const SLOP: f32 = 4.0;

        let (x, y) = self.pointer;
        let now = std::time::Instant::now();
        let count = match self.last_click {
            Some((when, at_x, at_y))
                if now.duration_since(when) < INTERVAL
                    && (at_x - x).abs() < SLOP
                    && (at_y - y).abs() < SLOP =>
            {
                // Wraps at three: a fourth click starts again at a caret, which is what every editor
                // does and what stops a rapid series from getting stuck selecting whole lines.
                (self.click_count % 3) + 1
            }
            _ => 1,
        };

        self.last_click = Some((now, x, y));
        count
    }

    /// A click in the sidebar: the tree, the settings panel, or nothing.
    fn click_sidebar(&mut self, sidebar: Rect, y: f32, scale: f32) {
        // The row heights each panel draws with. Kept beside the drawing code's own constants; a
        // mismatch here shows up as clicking one row and selecting its neighbour.
        if self.state.tree.is_some() {
            let top = sidebar.y + space::LG * scale + 22.0 * scale * 1.6;
            let row = ((y - top) / (22.0 * scale)).floor();
            if row < 0.0 {
                return;
            }
            let Some(tree) = &mut self.state.tree else {
                return;
            };
            let height = ((sidebar.bottom() - top) / (22.0 * scale)).floor().max(1.0) as usize;
            let (rows, _) = tree.visible(height);
            let Some(target) = rows.get(row as usize).map(|row| row.relative.clone()) else {
                return;
            };

            // Selecting by path rather than by index, because the window may be scrolled.
            tree.reveal(&target);
            if let Some(path) = tree.activate() {
                self.activate(Target::File(path));
            }
            return;
        }

        if self.state.settings.is_some() {
            let top = sidebar.y + space::LG * scale + 20.0 * scale * 1.7;
            let Some(settings) = &mut self.state.settings else {
                return;
            };
            // Option rows are two lines tall, import rows one and a bit; walking the list is more
            // honest than a formula that would drift from the drawing code.
            let mut cursor = top;
            for (index, entry) in vex::settings_ui::Settings::lines().iter().enumerate() {
                let block = match entry {
                    vex::settings_ui::Line::Option(_) => 20.0 * scale * 2.0,
                    _ => 20.0 * scale * 1.4,
                };
                let heading = if *entry
                    == vex::settings_ui::Line::Import(vex::settings_ui::Importable::Settings)
                {
                    20.0 * scale * 2.0
                } else {
                    0.0
                };
                cursor += heading;
                if y >= cursor && y < cursor + block {
                    settings.selected = index;
                    return;
                }
                cursor += block + 4.0 * scale;
            }
        }
    }

    /// A click in the results panel selects and opens that hit.
    fn click_results(&mut self, panel: Rect, y: f32, scale: f32) {
        let row = ((y - panel.y - 22.0 * scale) / (22.0 * scale)).floor();
        if row < 0.0 {
            return;
        }
        let Some(results) = &mut self.state.results else {
            return;
        };
        let (rows, _) = results.visible();
        if let Some(hit) = rows.get(row as usize).cloned() {
            results.selected = results
                .hits
                .iter()
                .position(|candidate| {
                    candidate.path == hit.path && candidate.line_number == hit.line_number
                })
                .unwrap_or(results.selected);
            self.open_hit();
        }
    }

    /// Picks up the folder dialog's answer, if it has one.
    fn poll_folder_picker(&mut self) -> bool {
        let Some(picker) = &self.folder_picker else {
            return false;
        };
        let Some(answer) = picker.poll() else {
            return false;
        };
        self.folder_picker = None;

        match answer {
            Some(folder) => {
                // The portal can hand back a *file* when the user double-clicks one in a directory
                // chooser, so the parent is taken rather than refusing.
                let folder = if folder.is_dir() {
                    folder
                } else {
                    folder.parent().map(|p| p.to_path_buf()).unwrap_or(folder)
                };
                self.open_folder(&folder);
            }
            None => self.state.hover = None,
        }
        true
    }

    /// Re-reads configuration after `init.lua` changes.
    ///
    /// Theme, metrics and key bindings are replaced; nothing about the *documents* is touched. A
    /// reload that reset scroll positions or dropped undo history would make editing your own config
    /// a hostile experience — which is exactly when this feature is used.
    fn reload_config(&mut self) {
        let settings = vex::config::load();

        self.state.theme = settings.theme;
        self.state.typography = settings.typography;
        self.state.format_on_save = settings.format_on_save;
        self.state.display.tab_width = settings.tab_width;
        self.state.display.mode = if settings.soft_wrap {
            WrapMode::Word
        } else {
            WrapMode::None
        };
        self.state.pairs = pair_config(self.state.file_path.as_deref(), settings.tab_width);
        self.state.modal = settings.modal;
        self.grammar.set_modal(settings.modal);
        if !settings.modal {
            self.state.mode = Mode::Insert;
        }
        self.state.sticky_scroll = settings.sticky_scroll;
        self.state.show_inlay_hints = settings.inlay_hints;
        self.state.setting_sources = settings.sources.clone();
        self.grammar.set_keymap(settings.keymap);

        // Wrap width is in columns and follows the font size, which may just have changed.
        if let Some(window) = &self.window {
            let scale = window.scale_factor() as f32;
            let width = window.inner_size().width as f32;
            fit_wrap_columns(&mut self.state, width, scale);
        }

        self.state.hover = Some(if settings.problems.is_empty() {
            "config reloaded".to_string()
        } else {
            for problem in &settings.problems {
                eprintln!("vex: config: {problem}");
            }
            std::iter::once(format!("config: {} problems", settings.problems.len()))
                .chain(settings.problems.iter().cloned())
                .collect::<Vec<_>>()
                .join("\n")
        });
        eprintln!("vex: reloaded configuration");
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
        let config_file = vex::config::config_dir().join("init.lua");

        for path in changed {
            // Config first: it is not a buffer, and reloading it is not reloading text.
            if vex_project::canonical(&path) == vex_project::canonical(&config_file) {
                self.reload_config();
                redraw = true;
                continue;
            }

            // A save this editor just made comes back as an event; ignoring it once is the whole
            // difference between a quiet save and a reload storm.
            if self.self_written.remove(&path) {
                continue;
            }

            let active = self
                .state
                .file_path
                .as_ref()
                .map(|open| vex_project::canonical(std::path::Path::new(open)));
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

    /// Opens the file tree, revealing the file being edited.
    ///
    /// Revealing rather than starting collapsed at the root: the tree is a way to look around from
    /// where you are, not a fresh start.
    fn show_tree(&mut self) {
        let mut tree =
            vex::tree::FileTree::new(self.index.root().to_path_buf(), self.index.files());
        if let Some(path) = &self.state.file_path {
            let absolute = vex_project::canonical(std::path::Path::new(path));
            if let Ok(relative) = absolute.strip_prefix(self.index.root()) {
                tree.reveal(&relative.to_string_lossy());
            }
        }
        self.state.tree = Some(tree);
        self.state.show_diagnostics = false;
    }

    /// Switches the project to another folder: re-indexes, and shows the tree.
    ///
    /// The open buffers stay open. Changing what `ctrl-p` searches should not close the file you were
    /// reading when you decided to go looking for another one.
    fn open_folder(&mut self, folder: &std::path::Path) {
        // Through `canonical`, so what the statusline shows is a path a person recognises rather than
        // Windows' `\\?\C:\...` form.
        let root = vex_project::canonical(folder);
        eprintln!("vex: project root {}", root.display());

        self.index = FileIndex::new(root.clone());
        self.walk = Some(FileWalk::start(root.clone()));
        self.state.project_root = root;
        self.state.results = None;
        self.show_tree();
        self.state.hover = Some(format!("opened {}", folder.display()));
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
        let absolute = vex_project::canonical(std::path::Path::new(&path));
        if let Ok(relative) = absolute.strip_prefix(&root) {
            tree.reveal(&relative.to_string_lossy());
        }
    }

    /// Resets per-buffer derived state after the active document changes.
    fn after_buffer_change(&mut self) {
        // The focused pane shows whatever is live. Without this it keeps drawing the buffer it
        // remembers, which is how choosing a file from the tree came to display an empty one.
        self.panes.follow(self.state.active_buffer);

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

        // The diff describes the document that just left.
        self.state.diff = vex::git::Diff::default();
        self.diffed_version = None;

        // Folds describe the document that just left. Keeping them would hide unrelated lines in
        // the new one, since they are line numbers and nothing more.
        self.state.display.folds.clear();

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

        // The wizard owns the window while it is up. Drawing the editor behind it would be showing
        // someone an interface while asking them how they want it to behave.
        if let Some(welcome) = &self.state.welcome {
            chrome::draw_welcome(
                &mut self.frame,
                welcome,
                &self.state.typography,
                Rect::new(0.0, 0.0, width as f32, height as f32),
                scale,
            );
            if let Err(err) = renderer.render(&self.frame, welcome.preview_theme().bg) {
                eprintln!("vex: frame dropped: {err:#}");
            }
            return;
        }

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

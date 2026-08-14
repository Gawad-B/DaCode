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

/// Columns of context kept beside the cursor when the view scrolls sideways.
///
/// Eight, where the vertical margin is three: a row of context is a whole line, a column is one
/// character, and a caret pinned to the exact right edge shows nothing of what it is typing
/// towards. `scroll_to_reveal_column` caps it at half the viewport, so a narrow pane still works.
const H_SCROLL_MARGIN: usize = 8;

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
    let named_something = argument.is_some();
    let path = if folder.is_some() { None } else { argument };

    // With no file named — including when a folder was — the last session for that project is what
    // to open. Naming a file explicitly wins: asking for one file and getting the last three back
    // would be surprising.
    // Started with nothing at all — from a Start Menu entry, a desktop icon, a dock — the project to
    // open is the last one worked in, not whatever directory the launcher happened to point at. On
    // Windows that directory is where the program was installed, which is nobody's project.
    let remembered = (!named_something && !args.iter().any(|arg| arg == "--no-session"))
        .then(|| vex::session::last_project(&vex::session::session_dir()))
        .flatten();

    let restore = (path.is_none() && !args.iter().any(|arg| arg == "--no-session"))
        .then(|| {
            let anchor = folder
                .clone()
                .or_else(|| remembered.clone())
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
    let root = match (&folder, &remembered) {
        (Some(folder), _) => folder.clone(),
        // Nothing named and nothing opened from a session: the last project is still the right place
        // to be, even if its session has since been emptied.
        (None, Some(remembered)) if file_path.is_none() => remembered.clone(),
        _ => find_root(&std::path::PathBuf::from(
            file_path.clone().unwrap_or_else(|| ".".to_string()),
        )),
    };
    eprintln!("vex: project root {}", root.display());
    vex::session::remember_project(&vex::session::session_dir(), &root);

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

    let mut app = App::new(root.clone(), state, grammar, &starting_settings);
    app.walk = Some(FileWalk::start(root.clone()));

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
        app.restore_session(&session);
        app.restore_window = session.window;
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

    let tab_name = path.last().cloned().unwrap_or_else(|| "untitled".into());

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
        // The one open buffer. `App` keeps this in step from `after_buffer_change`; seeding it here
        // is what makes the offscreen capture path — which has no `App` — draw a real tab.
        buffer_tabs: vec![(tab_name, false)],
        active_buffer: 0,
        modal: settings.modal,
        sticky_scroll: settings.sticky_scroll,
        recording: None,
        diff: vex::git::Diff::default(),
        show_diagnostics: false,
        problems: Default::default(),
        inlay_hints: Vec::new(),
        show_inlay_hints: settings.inlay_hints,
        welcome: None,
        settings: None,
        setting_sources: settings.sources.clone(),
        tree: None,
        autosave: settings.autosave,
        line_numbers: settings.line_numbers,
        pointer: (0.0, 0.0),
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
const VALUE_FLAGS: [&str; 15] = [
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
    "--size",
    "--at-column",
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
///
/// Split on **both** separators. Windows hands the editor `crates\vex\src\main.rs`, and splitting on
/// `/` alone made that one crumb — so the title bar showed a raw path where every other platform
/// shows breadcrumbs. Nothing failed; it just looked wrong, and only on the platform nobody was
/// looking at. Found by rendering a screenshot there.
fn relative_crumbs(path: &str) -> Vec<String> {
    let all: Vec<String> = path
        .trim_start_matches("./")
        .trim_start_matches(".\\")
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
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

    match vex::open::read(std::path::Path::new(&path)) {
        vex::open::Opened::Text(contents) => {
            let crumbs = relative_crumbs(&path);
            Ok((Text::from_str(&contents), crumbs, Some(path)))
        }
        // `vex newfile.rs` is meant to work, so a path that does not exist yet is a buffer that
        // knows its name. Anything else — too large, binary, another encoding — keeps `file_path`
        // as `None`, because a buffer that could be saved over a file it never managed to read is
        // how an editor destroys something.
        vex::open::Opened::Refused(why) => {
            let missing = !std::path::Path::new(&path).exists();
            if missing {
                Ok((Text::from_str(""), relative_crumbs(&path), Some(path)))
            } else {
                eprintln!("vex: {why}");
                Ok((Text::from_str(""), vec!["untitled".to_string()], None))
            }
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

    // `--size WxH` renders at a chosen size. The harness was fixed at 1600x1000, which is why a
    // whole class of defect had never appeared in a capture: the settings rows only collide with the
    // footer below about 900px of client height, and the toolbar only runs off the left edge below
    // about 450px of width. Both were reported from a real window and neither was reproducible here.
    let (width, height) = value("--size")
        .and_then(|size| {
            let (w, h) = size.split_once(['x', 'X'])?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1600u32, 1000u32));
    let (width, height) = (width.max(200), height.max(200));

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
        // The harness seeds a half-typed `3d` into the pending cell to show the statusline doing its
        // job. There is no operator-count to be half-way through without the grammar, so every
        // `--no-vim` capture was showing a Vim command in progress that could not exist.
        state.pending.clear();
    }

    // `--at-column N` parks the view sideways, with wrap off. Without it the horizontal axis is
    // unrenderable, and an axis that cannot be captured is one whose defects ship — which is
    // exactly what happened to the four overflow bugs before `--size` existed.
    if let Some(column) = value("--at-column").and_then(|v| v.parse::<usize>().ok()) {
        state.display.mode = WrapMode::None;
        state.scroll.column = column;
    }

    if let Some(line) = value("--at").and_then(|v| v.parse::<usize>().ok()) {
        let text = state.document.text();
        let line = line.min(text.last_line());
        let offset = text.point_to_char(vex_text::Point::new(line, 0));
        state.document.set_primary_caret(offset);
        state.scroll = ScrollAnchor::at_line(line.saturating_sub(2));
    }

    // `--block LINE:COLUMN` draws a rectangular selection from the cursor to there, so the one
    // gesture that has no key can still be captured. Over ragged text on purpose: whether short
    // lines take part is the thing worth looking at.
    if let Some(corner) = value("--block") {
        let (line, column) = corner.split_once(':').unwrap_or((corner.as_str(), "0"));
        let text = state.document.text();
        let head = vex_text::Point::new(
            line.parse::<usize>().unwrap_or(0).min(text.last_line()),
            column.parse::<usize>().unwrap_or(0),
        );
        let anchor = text.char_to_point(state.cursor());
        state
            .document
            .set_selections(vex_text::SelectionSet::column(text, anchor, head));
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
        let items: Vec<vex_lsp::lsp_types::Diagnostic> = sample
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
        // The project store as well as the active file's, and a second file so the capture shows
        // what the panel is actually for: problems somewhere other than where you are looking.
        if let Some(path) = state.file_path.clone() {
            state
                .problems
                .publish(std::path::Path::new(&path), &items, None);
        }
        state.problems.publish(
            &state.project_root.join("crates/vex-buffer/src/document.rs"),
            &items[..2],
            None,
        );
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
        state.mutate(|document| document.insert(&format!(" {prefix}")));
        let caret = state.cursor();
        let mut session = vex::completion::Completion::new(
            state.document.text(),
            caret,
            state.document.version(),
        );
        // Shaped like a real rust-analyzer reply — kinds, types, a snippet and a deprecation —
        // because a fixture of bare labels cannot show whether the type column fits, which is the
        // half of the popup this capture exists to check.
        let suggest = |label: &str, kind: &'static str, detail: &str| vex_lsp::CompletionItem {
            label: label.to_string(),
            insert: label.to_string(),
            detail: (!detail.is_empty()).then(|| detail.to_string()),
            kind,
            ..Default::default()
        };
        session.offer(
            vec![
                suggest("char_to_point", "method", "fn(&self, CharOffset) -> Point"),
                suggest(
                    "char_to_byte",
                    "method",
                    "fn(&self, CharOffset) -> ByteOffset",
                ),
                suggest("char_to_line", "method", "fn(&self, CharOffset) -> usize"),
                suggest("chars", "method", "fn(&self) -> Chars<'_>"),
                suggest("clamp", "fn", "fn(self, Self, Self) -> Self"),
                vex_lsp::CompletionItem {
                    deprecated: true,
                    ..suggest("clone", "method", "fn(&self) -> Self")
                },
            ],
            false,
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
        state.menu = Some(vex::menu::Menu::bar(state.modal));
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--context-menu") {
        state.menu = Some(vex::menu::Menu::context((420.0, 300.0), state.modal));
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if let Some(prefix) = value("--which-key") {
        // The popup is driven by the pending display, exactly as a half-typed chord drives it.
        state.pending = prefix;
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if let Some(query) = value("--symbols") {
        // A canned outline, so the palette's indenting and its kind column are inspectable without
        // a language server — the same reason `--complete` has a canned list.
        let mut palette = vex::palette::Palette::new(vex::palette::PaletteKind::Symbols);
        let at = |line: u32| vex_lsp::lsp_types::Position {
            line,
            character: 11,
        };
        let symbol =
            |name: &str, kind: &'static str, container: &str, depth, line| vex_lsp::Symbol {
                name: name.to_string(),
                kind,
                container: container.to_string(),
                path: std::path::PathBuf::from(FIXTURE),
                position: at(line),
                depth,
            };
        let symbols = vec![
            symbol("Bias", "enum", "", 0, 12),
            symbol("Left", "variant", "Bias", 1, 15),
            symbol("Right", "variant", "Bias", 1, 18),
            symbol("Anchor", "struct", "", 0, 22),
            symbol("offset", "field", "Anchor", 1, 23),
            symbol("bias", "field", "Anchor", 1, 24),
            symbol("new", "method", "Anchor", 1, 28),
            symbol("left", "method", "Anchor", 1, 32),
            symbol("right", "method", "Anchor", 1, 36),
            symbol("map_through", "method", "Anchor", 1, 41),
        ];
        palette.query = query;
        palette.refresh_symbols(&symbols, true);
        state.palette = Some(palette);
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--commands") {
        let mut palette = vex::palette::Palette::new(vex::palette::PaletteKind::Commands);
        palette.refresh_commands(state.modal);
        state.palette = Some(palette);
        chrome::draw(&mut frame, &state, bounds, 1.0);
    } else if flag("--settings") {
        let mut settings = vex::settings_ui::Settings::new();
        // The real detection, against this machine — the section is a report, and a screenshot of a
        // report with invented contents would verify nothing about whether it can say the true
        // thing. `--at` scrolls to it, since it sits below the fold at every size.
        let root = find_root(&std::path::PathBuf::from("."));
        let home = vex::paths::home();
        let loaded = vex::config::load();
        settings.servers = vex::settings_ui::detect(&root, Some(&home), &loaded.servers);
        settings.formatters =
            vex::settings_ui::detect_formatters(&root, Some(&home), &loaded.formatters);
        settings.selected = value("--row")
            .and_then(|at| at.parse::<usize>().ok())
            .map(|at| at.min(settings.lines().len() - 1))
            .unwrap_or(0);
        state.settings = Some(settings);
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
                    column: 0,
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

/// Whether an offset falls inside a non-empty selection.
///
/// A free function so it can be tested without a window: the rule it encodes — a right-click inside
/// a selection keeps it — is the difference between a context menu whose `cut` and `copy` mean
/// something and one where they silently take a single character.
fn selection_contains(selections: &vex_text::SelectionSet, offset: vex_text::CharOffset) -> bool {
    selections
        .iter()
        .any(|selection| !selection.is_caret() && selection.range().contains(&offset))
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
    /// Servers named by `vex.server(...)` in `init.lua`, tried before the ones vex knows about.
    configured_servers: Vec<vex_lsp::Configured>,
    /// Whether each running server answers `textDocument/formatting`, keyed by program.
    ///
    /// Asked at the handshake rather than discovered by trying: pyright says no, and an editor that
    /// asks anyway gets silence and has nothing to tell the user — where knowing means it can run
    /// black instead.
    server_formats: std::collections::HashMap<String, bool>,
    /// Formatters named by `vex.formatter(...)`, tried before the ones vex knows about.
    configured_formatters: Vec<vex::format::Configured>,
    /// Runs external formatters off the UI thread.
    formatter: vex::format::FormatWorker,
    /// The version an external format was requested against, so a stale reply is refused.
    formatting_version: Option<vex_text::Version>,
    /// Languages already reported as having no server, so the notice is shown once per session
    /// rather than every time a file of that language is opened.
    missing_servers: std::collections::HashSet<String>,
    /// The file a delete was just refused for, so choosing it again goes through.
    confirm_delete: Option<String>,
    /// The buffer a close was just refused for, so pressing close again goes through.
    confirm_close: Option<usize>,
    /// The size the last session's window had, applied when this one is created.
    restore_window: Option<(u32, u32)>,
    /// Whether closing the window has already been refused once for unsaved work.
    confirm_quit: bool,
    /// The document version last sent to the server, so `didChange` is not sent redundantly.
    sent_version: Option<vex_text::Version>,
    /// The server, and the document it currently believes is open on it.
    ///
    /// Every `didOpen` owes a `didClose`, and none was ever sent. The editor announced each file it
    /// visited and never took one back, so a server accumulated every document of a whole session —
    /// each frozen at the version last sent, each still answering requests and publishing
    /// diagnostics for a buffer that may have been closed an hour before.
    opened_document: Option<(String, std::path::PathBuf)>,
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
    /// The symbols behind the outline palette, whichever scope it was opened at.
    symbols: Vec<vex_lsp::Symbol>,
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
    /// Where an alt-drag started, in buffer coordinates.
    ///
    /// Recorded separately from the selection set because the alt-*click* that begins the gesture
    /// has already added a cursor by the time the pointer moves, so the set no longer says where
    /// the rectangle's corner was.
    block_origin: Option<vex_text::Point>,
    /// When and where the last click landed, for recognising a double or triple one.
    ///
    /// The platform does not tell us: `winit` reports presses, and the "is this the second click of
    /// a double?" question is the application's. Both halves are needed — a second click a second
    /// later, or an inch away, is a new click and not a word selection.
    last_click: Option<(std::time::Instant, f32, f32)>,
    /// 1, 2 or 3: caret, word, line. Kept for the whole gesture so a drag after a double-click is
    /// still a word-wise drag.
    click_count: u8,
    /// When the document was last changed, so autosave can wait for a pause in typing rather than
    /// writing a half-typed word to disk every few seconds.
    last_edit: Option<std::time::Instant>,
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

        // `mut` only where something mutates it. The X11 and Wayland names below are the only thing
        // that does, and they are Unix-only — so on Windows this was an unused `mut`, a warning that
        // clippy would have failed the build on if clippy had ever run there. It runs on Linux in CI,
        // where the `mut` is used, so nothing saw it until the workspace was built on Windows.
        // The size the last session ended at, in physical pixels — the stored value came from
        // `inner_size`, so it must go back as a physical size or a scaled display would grow the
        // window a little on every launch.
        let size = match self.restore_window {
            Some((width, height)) => winit::dpi::Size::Physical((width, height).into()),
            None => winit::dpi::Size::Logical((1280.0, 820.0).into()),
        };

        #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(unused_mut))]
        let mut attributes = Window::default_attributes()
            .with_title("vex")
            .with_window_icon(vex::icon::window_icon())
            .with_inner_size(size);

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
        changed |= self.poll_formatter();
        changed |= self.sync_git();
        changed |= self.handle_file_changes();
        changed |= self.poll_folder_picker();
        changed |= self.poll_autosave();
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
                // Closing the window with unsaved work used to exit immediately. Recovery would have
                // held it — it is written on the way out — but "your work is in a recovery file
                // somewhere" is not an answer anybody wants to be given by a window that has already
                // gone.
                //
                // The same shape as closing a buffer: refuse once, say what saves it, and let the
                // second attempt through. A dialog would be better and is a larger change; this
                // cannot lose anything, which is the property that matters.
                let unsaved = self.unsaved_count();
                if unsaved > 0 && !self.confirm_quit {
                    self.confirm_quit = true;
                    self.state.hover = Some(format!(
                        "{unsaved} unsaved file{} — {} saves this one, or close again to leave them \
                         in the recovery file",
                        if unsaved == 1 { "" } else { "s" },
                        self.key_for("save")
                    ));
                    self.request_redraw();
                    return;
                }
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
                let was_over_toolbar = self.over_toolbar();
                self.pointer = (position.x as f32, position.y as f32);
                self.state.pointer = self.pointer;
                if let Some(welcome) = &mut self.state.welcome {
                    welcome.pointer = self.pointer;
                    self.request_redraw();
                }

                // An open menu owns the pointer: the row under it lights up, and a drag must not
                // reach the buffer behind a card that is covering it.
                //
                // There was no branch here at all, so `selected` never moved and the first row
                // stayed lit wherever the pointer went. A menu that does not respond to the mouse
                // at all reads as a menu that cannot be clicked — which is exactly how it was
                // reported, even though the hit test underneath was correct the whole time.
                if self.state.menu.is_some() {
                    let (x, y) = self.pointer;
                    let (bounds, scale) = (self.window_bounds(), self.scale());
                    let mut moved = false;
                    if let Some(menu) = &mut self.state.menu {
                        let geometry = menu.geometry(bounds, scale);
                        if let Some(index) = menu.row_at(&geometry, x, y) {
                            moved = menu.selected != index;
                            menu.selected = index;
                        }
                    }
                    if moved {
                        self.request_redraw();
                    }
                    return;
                }

                // Dragging extends the selection, which is how every editor spells "select this".
                if self.dragging {
                    self.click(true);
                    self.request_redraw();
                } else if was_over_toolbar || self.over_toolbar() {
                    // Redraw only when a toolbar button lights up or stops being lit. Repainting on
                    // every pointer move would spend a frame per mouse event on a window where
                    // nothing changed — the one rule this editor has.
                    self.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                match (button, state) {
                    (winit::event::MouseButton::Left, ElementState::Pressed) => {
                        // Choosing a menu row is not the start of a drag. Leaving `dragging` set
                        // meant the smallest jitter before the button came up re-entered
                        // `click(true)`, which skips the menu branch, lands in the pane behind it
                        // and replaces the selection — visibly undoing whatever the row had just
                        // done, a moment after it did it.
                        self.dragging = self.state.menu.is_none() && self.state.welcome.is_none();
                        self.click_count = self.count_click();
                        self.click(false);
                    }
                    (winit::event::MouseButton::Left, ElementState::Released) => {
                        self.dragging = false;
                        self.block_origin = None;
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
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(position) => {
                        (position.x as f32 / 40.0, position.y as f32 / 40.0)
                    }
                };

                // ctrl-scroll resizes the text, as it does in every browser and every other editor.
                // The wheel handler ignored modifiers entirely, so the one gesture people try first
                // when the type is too small did nothing.
                if self.modifiers.control_key() {
                    if dy != 0.0 {
                        let _ = self.run(if dy > 0.0 {
                            Action::IncreaseFontSize
                        } else {
                            Action::DecreaseFontSize
                        });
                        self.request_redraw();
                    }
                    return;
                }

                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y as isize * 3,
                    MouseScrollDelta::PixelDelta(position) => -(position.y / 40.0) as isize,
                };
                if lines != 0 {
                    self.scroll_by(lines);
                    self.sync_syntax();
                }

                // Sideways. A trackpad and a tilt wheel send an x delta directly; shift plus a
                // plain wheel is the conventional way to send one from a mouse that cannot. Reading
                // only y, and ignoring the modifier, is why a horizontal gesture did nothing.
                let sideways = if self.modifiers.shift_key() && dx == 0.0 {
                    -dy
                } else {
                    -dx
                };
                if sideways != 0.0 && self.state.display.mode == WrapMode::None {
                    let columns = (sideways * 6.0) as isize;
                    self.scroll_sideways(columns);
                }

                self.request_redraw();
            }

            // Dropping a file on the window opens it. Nothing handled this, so dragging a file onto
            // vex did nothing at all — which reads as "this is a toy" faster than any missing
            // feature does.
            WindowEvent::DroppedFile(path) => {
                self.activate(Target::File(path));
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
                    // The same refusal `CloseRequested` makes, for the same reason: ctrl-q with
                    // three dirty buffers must not be one keystroke away from leaving them.
                    let unsaved = self.unsaved_count();
                    if unsaved > 0 && !self.confirm_quit {
                        self.confirm_quit = true;
                        self.state.hover = Some(format!(
                            "{unsaved} unsaved file{} — {} saves this one, or quit again to leave \
                             them in the recovery file",
                            if unsaved == 1 { "" } else { "s" },
                            self.key_for("save")
                        ));
                        self.request_redraw();
                        return;
                    }
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
    /// Builds the editor's state machine, with no window and no renderer.
    ///
    /// **One construction site**, shared by `run` and by the tests, so a test cannot be exercising
    /// a differently-assembled editor from the real one. The window and the renderer are already
    /// `Option` because winit creates them on `resumed` — and that is what makes the whole machine
    /// drivable headlessly: everything below `window_event` is ordinary code over ordinary data.
    ///
    /// Worth having for one reason. An adversarial review of this layer found thirteen defects the
    /// tests could not have caught, because every test stopped at the pure functions the wiring
    /// calls and none of them drove the wiring: a formatter that never applied its result, a revert
    /// that could overwrite a different file, a flag that made a later unrelated edit save a buffer
    /// nobody asked to save. Each is one `poll` and one assertion away from being caught.
    fn new(
        root: std::path::PathBuf,
        state: EditorState,
        grammar: Grammar,
        settings: &vex::config::Settings,
    ) -> Self {
        Self {
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
            configured_servers: settings.servers.clone(),
            server_formats: std::collections::HashMap::new(),
            configured_formatters: settings.formatters.clone(),
            formatter: vex::format::FormatWorker::spawn(),
            formatting_version: None,
            missing_servers: std::collections::HashSet::new(),
            confirm_close: None,
            confirm_delete: None,
            restore_window: None,
            confirm_quit: false,
            sent_version: None,
            opened_document: None,
            folder_picker: None,
            snippets: load_snippets(),
            snippet: None,
            jumps: vex_input::JumpList::default(),
            macros: vex_input::Macros::default(),
            code_actions: Vec::new(),
            symbols: Vec::new(),
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
            block_origin: None,
            last_click: None,
            click_count: 1,
            last_edit: None,
            wakes: 0,
            index: FileIndex::new(root),
            // The project walk is started by `run`, not here: a test wants an editor, not an
            // indexing thread over whatever directory it was handed.
            walk: None,
            buffers: Vec::new(),
            state,
        }
    }

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
        // Minus whatever sticky scroll has pinned above the viewport. This is the height the cursor
        // reveal and the page motions work in, and without the subtraction they treated rows the
        // pinned strip is covering as visible — so the caret could be "revealed" underneath it.
        let pinned = self
            .state
            .sticky_heads(self.state.document.text(), self.state.scroll.line, true)
            .len() as f32
            * row_height;
        self.state
            .visible_rows((code_height - pinned).max(row_height), row_height)
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

        // And sideways. Without this, with soft wrap off, the caret walks off the right edge and
        // the text it is in is both invisible and unreachable — the defect this whole axis exists
        // for. Inert while wrapping, since there is nothing to the right of the viewport then.
        let width = self.focused_grid().columns();
        self.state.scroll = self.state.display.scroll_to_reveal_column(
            self.state.document.text(),
            self.state.scroll,
            width,
            cursor,
            H_SCROLL_MARGIN,
        );
    }

    /// Slides the view sideways, clamped to what is actually on screen.
    ///
    /// Clamped against the longest *visible* row rather than the longest in the file: the bound is
    /// per-frame work and must stay bounded by the viewport, and scrolling past the end of what you
    /// can see into empty space is not useful anyway.
    fn scroll_sideways(&mut self, columns: isize) {
        if self.state.display.mode != WrapMode::None {
            return;
        }
        let rows = self.code_rows().max(1);
        let visible =
            self.state
                .display
                .rows_from(self.state.document.text(), self.state.scroll, rows);
        let longest = visible.iter().map(|row| row.len()).max().unwrap_or(0);

        let current = self.state.scroll.column as isize;
        self.state.scroll.column = current
            .saturating_add(columns)
            .clamp(0, longest.saturating_add(8) as isize)
            as usize;
    }

    fn scroll_by(&mut self, rows: isize) {
        let text = self.state.document.text().clone();
        // The walk lives in the display map, which is the only thing that knows what a fold hides.
        // This function kept its own copy of it and did not, so the wheel stepped over closed
        // blocks a line at a time and could not climb back up over one.
        self.state.scroll = self.state.display.step(&text, self.state.scroll, rows);
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

        // Through `from_platform`, which lowercases a character that has ctrl or alt on it: the
        // window system applies shift before we see it, so `ctrl-shift-p` arrives as `P` and every
        // binding spelled `Char('p') if shift` misses. Shift is still on the key for the bindings
        // that mean it.
        Some(Key::from_platform(
            code,
            self.modifiers.control_key(),
            self.modifiers.alt_key(),
            self.modifiers.shift_key(),
        ))
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
            // The key that opened it closes it. Only escape did, which is fine if you know that and
            // a trap if you pressed f10 to find out what it was.
            KeyCode::Escape | KeyCode::Function(10) => self.state.menu = None,
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
            KeyCode::Escape | KeyCode::Function(1) => self.state.help = None,
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
        //
        // Not when the pointer is *inside* a selection, though, which every editor reads as "this is
        // what I mean". Moving the caret there collapsed it, so `cut` and `copy` — the two rows
        // anybody opens a right-click menu for — ran against an empty range and did nothing at all.
        // The menu looked unclickable when what was broken was the click that opened it.
        // On the tree, a different menu: a right-click is about what is under the pointer, and
        // there it is a file rather than a symbol.
        let layout = self.layout();
        if self.state.tree.is_some()
            && vex::layout::contains(layout.sidebar, self.pointer.0, self.pointer.1)
        {
            if self.state.menu.is_none() {
                // Select the row first, so the menu acts on what was clicked.
                self.click_sidebar(layout.sidebar, self.pointer.1, self.scale());
            }
            self.state.menu = Some(vex::menu::Menu::tree(self.pointer, self.state.modal));
            return;
        }

        if self.state.menu.is_none() && !self.pointer_is_in_a_selection() {
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
        let version_before = self.state.document.version();

        let outcome = execute(&mut self.state, &mut self.registers, &action, rows);
        if let Outcome::Window(action) = &outcome {
            self.window_action(action);
        }

        // The clock autosave waits on. Read from the document's version rather than from a list of
        // actions that change text, for the same reason the clipboard is: a list would be wrong the
        // first time an action was added.
        if self.state.document.version() != version_before {
            self.last_edit = Some(std::time::Instant::now());
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
        // The buffer that was live has stopped being the document the server is told about, so it
        // is closed before the next one is opened. This is the single hook for it: switching
        // buffers, closing one, renaming the file out from under it and deleting it all reach here
        // through `after_buffer_change`.
        self.close_open_document();
        self.sent_version = None;
        self.state.diagnostics.clear();

        let Some(path) = self.state.file_path.clone() else {
            self.active_server = None;
            self.state.server_status = "scratch buffer".into();
            return;
        };
        let candidates = vex_lsp::servers_for(&path, &self.configured_servers);
        let Some(first) = candidates.first().cloned() else {
            self.active_server = None;
            self.state.server_status = "no server for this language".into();
            return;
        };

        // Already running for another buffer of the same language: switching to a TypeScript file
        // and back to Rust must not restart rust-analyzer, which would throw away minutes of
        // indexing every time you glance at another file.
        if let Some(running) = candidates
            .iter()
            .find(|spec| self.servers.contains_key(&spec.program))
        {
            self.active_server = Some(running.program.clone());
            self.state.server_status = running.program.clone();
            return;
        }

        // The first one that is actually on this machine, config's choices first. "The Python
        // server" is not one program — pyright comes from npm and pylsp from pip — so offering only
        // the first and reporting it missing is how an editor tells someone with a working language
        // server that it cannot find one.
        let root = self.index.root().to_path_buf();
        let home = vex::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        let found = candidates.iter().find_map(|spec| {
            vex_lsp::resolve_command(&spec.program, &root, home.as_deref()).map(|at| (spec, at))
        });

        let Some((spec, executable)) = found else {
            self.active_server = None;
            // Names the *language*, not a binary nobody has heard of. The old text was
            // `pyright-langserver not installed`, which someone with Python installed reads —
            // reasonably — as "vex cannot see my Python".
            self.state.server_status = format!("no {} server", first.language.to_lowercase());

            // The statusline cell is one narrow line, so the part that actually helps — what vex
            // looked for and how to install it — goes in the info box, the same place config
            // problems are reported. Once per language per session: useful the first time you open
            // a .py file, nagging by the third.
            let names = candidates
                .iter()
                .map(|spec| spec.program.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if self.missing_servers.insert(first.language.clone()) {
                // A server the user named is a different failure from one vex only guessed at:
                // theirs is a path that is wrong, not a program that is missing, and telling them
                // to install pyright when they asked for a venv would be answering the wrong
                // question.
                self.state.hover = Some(if first.configured {
                    format!(
                        "init.lua asks for `{}` for {}, and vex could not find it. Relative paths \
                         are resolved against the project root ({}). Nothing else served this \
                         file, so vex looked for {names}.",
                        first.program,
                        first.language,
                        root.display()
                    )
                } else {
                    format!(
                        "no {} language server. A language server is separate from the {} you have \
                         installed — vex looked for {names} on PATH. Install one with: {}. To name \
                         your own: vex.server(\"{}\", {{ command = \"…\" }})",
                        first.language, first.language, first.install, first.language_id
                    )
                });
            }
            eprintln!(
                "vex: no {} language server; looked for {names}",
                first.language
            );
            return;
        };

        let arguments: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        match vex_lsp::LspClient::start(&executable, &spec.program, &arguments, &root) {
            Ok(client) => {
                eprintln!(
                    "vex: starting {} ({}) in {}",
                    spec.program,
                    executable.display(),
                    root.display()
                );
                self.state.server_status = format!("{} starting", spec.program);
                self.servers.insert(spec.program.clone(), client);
                self.active_server = Some(spec.program.clone());
            }
            // Resolution succeeded and the spawn still failed: this is the case where the user is
            // right and vex is wrong, and it should read that way rather than blaming their PATH.
            Err(error) => {
                eprintln!("vex: {error:#}");
                self.active_server = None;
                self.state.server_status = format!("{} found but would not start", spec.program);
            }
        }
    }

    /// Whether anything at all could format the buffer on screen.
    ///
    /// Asked before `format_on_save` queues a request, so a save in a project with neither a
    /// formatting server nor a known formatter does not put one in flight that can only fail —
    /// and, more importantly, does not report a failure on every save of a file nobody asked to
    /// format.
    fn can_format(&self) -> bool {
        let by_server = self
            .active_server
            .as_ref()
            .and_then(|name| self.server_formats.get(name))
            .copied()
            .unwrap_or(false);
        by_server
            || self.state.file_path.as_ref().is_some_and(|path| {
                !vex::format::formatters_for(path, &self.configured_formatters).is_empty()
            })
    }

    /// Sends a pending format to whichever thing can actually do it.
    ///
    /// Run before the language-server sync, and it takes the request away when the answer is a
    /// program: `sync_lsp` returns immediately when no server is running, so a format left in
    /// `pending_request` would sit there forever in exactly the projects — plain JavaScript, CSS,
    /// Markdown — where an external formatter is the whole point.
    ///
    /// The server wins when it says it formats, because it is already running and already agrees
    /// with the project's own configuration. pyright says it does not, which is why this is a
    /// question asked at the handshake rather than assumed.
    fn route_format(&mut self) {
        if !matches!(self.state.pending_request, Some(Action::Format)) {
            return;
        }
        let by_server = self
            .active_server
            .as_ref()
            .and_then(|name| self.server_formats.get(name))
            .copied()
            .unwrap_or(false);
        if by_server {
            return;
        }

        self.state.pending_request = None;
        self.format_externally();
    }

    /// Runs the first formatter that is actually on this machine, on the worker.
    fn format_externally(&mut self) {
        // Taken up front and restored only when a request actually goes out. Every early return
        // below is a format that will never happen, and a `save_after_format` left set behind one
        // is a flag with no owner — the next server edit anywhere in the editor would see it and
        // write *that* buffer to disk unasked.
        let pending_save = std::mem::take(&mut self.save_after_format);

        let Some(path) = self.state.file_path.clone() else {
            self.warn("a scratch buffer has no language to format");
            return;
        };
        let candidates = vex::format::formatters_for(&path, &self.configured_formatters);
        let Some(first) = candidates.first().cloned() else {
            self.warn("vex knows no formatter for this file — name one with vex.formatter(…)");
            return;
        };

        let root = self.index.root().to_path_buf();
        let home = vex::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        let found = candidates.iter().find_map(|formatter| {
            vex::format::resolve(formatter, &root, home.as_deref())
                .map(|at| (formatter.clone(), at))
        });

        let Some((formatter, executable)) = found else {
            // Names the language and every program it looked for, the same shape as the missing
            // language-server notice — "no formatter" is not something anyone can act on.
            let names = candidates
                .iter()
                .map(|formatter| formatter.program.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            self.state.hover = Some(if first.configured {
                format!(
                    "init.lua asks for `{}` to format {}, and vex could not find it.",
                    first.program, first.language
                )
            } else {
                format!(
                    "no {} formatter — vex looked for {names} on PATH. Install one with: {}",
                    first.language, first.install
                )
            });
            self.state.server_status = format!("no {} formatter", first.language.to_lowercase());
            return;
        };

        let version = self.state.document.version();
        self.formatting_version = Some(version);
        self.save_after_format = pending_save;
        self.state.server_status = format!("{} formatting", formatter.program);
        self.formatter.request(vex::format::Request {
            formatter,
            executable,
            path: std::path::PathBuf::from(&path),
            root,
            text: self.state.document.text().to_string(),
            version,
        });
    }

    /// Applies a finished external format, if it is still for the text on screen.
    fn poll_formatter(&mut self) -> bool {
        let Some(formatted) = self.formatter.poll() else {
            return false;
        };
        // Taken, not cleared: the check below needs to know which version was asked for, and
        // clearing the field first made that comparison `None != Some(v)` — unconditionally true,
        // so every external format was refused and the feature never applied a single edit.
        let requested = self.formatting_version.take();

        if let Some(problem) = formatted.problem {
            self.state.server_status = "format failed".into();
            self.state.hover = Some(problem);
            self.save_after_format = false;
            return true;
        }
        let Some(text) = formatted.text else {
            return true;
        };

        // The one rule, applied to the newest producer: text formatted against a document the user
        // has typed into since is discarded, not merged. Saying so matters — the alternative is a
        // format that appears to have done nothing.
        if requested != Some(formatted.version)
            || self.state.document.version() != formatted.version
        {
            self.state.server_status = "format refused: the file changed".into();
            self.save_after_format = false;
            return true;
        }

        let before = self.state.document.text().to_string();
        match vex::format::narrow(&before, &text) {
            // Nothing to change, so nothing to write — and *not* saving again is the point. The
            // file on disk is already what the formatter would produce, and saving would re-arm
            // `format_on_save`, which would format, which would find nothing, which would save:
            // a loop with no fixed point. The language-server path already returns here for the
            // same reason.
            None => {
                self.state.server_status = "already formatted".into();
                self.save_after_format = false;
            }
            Some((range, replacement)) => {
                let edit = vex_text::Edit::replace(
                    vex_text::CharOffset(range.start)..vex_text::CharOffset(range.end),
                    replacement,
                );
                let applied = self.state.mutate(|document| {
                    document.apply(
                        vex_text::Transaction::new(vec![edit]),
                        vex_buffer::EditKind::Discrete,
                    )
                });
                match applied {
                    Ok(_) => self.state.server_status = "formatted".into(),
                    Err(error) => {
                        self.state.hover = Some(format!("formatting could not apply: {error}"));
                        self.save_after_format = false;
                    }
                }
            }
        }

        // Save-after-format, the same handshake the language-server path uses: `format_on_save`
        // formats first and writes what came back, or the file on disk is the unformatted one.
        if std::mem::take(&mut self.save_after_format) {
            self.write_active(false);
        }
        self.reveal_cursor();
        true
    }

    /// What the settings panel reports about language servers.
    ///
    /// Gathered when the panel opens rather than while drawing: it walks `PATH` once per program,
    /// which is bounded but not free.
    fn detect_servers(&self) -> Vec<vex::settings_ui::ServerRow> {
        let home = vex::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        vex::settings_ui::detect(self.index.root(), home.as_deref(), &self.configured_servers)
    }

    /// The same, for external formatters.
    fn detect_formatters(&self) -> Vec<vex::settings_ui::ServerRow> {
        let home = vex::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        vex::settings_ui::detect_formatters(
            self.index.root(),
            home.as_deref(),
            &self.configured_formatters,
        )
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
    /// Tells the server that the document it was told about is no longer open.
    ///
    /// Quietly does nothing when there is none, when the server has gone away, or when it never
    /// finished its handshake — a `didClose` for a document a server never heard about is worse
    /// than none, and there is nothing useful to say on screen about either case.
    fn close_open_document(&mut self) {
        let Some((server, path)) = self.opened_document.take() else {
            return;
        };
        let Some(client) = self.servers.get_mut(&server) else {
            return;
        };
        if client.state() == vex_lsp::State::Ready {
            if let Err(error) = client.did_close(&path) {
                eprintln!("vex: didClose failed: {error:#}");
            }
        }
    }

    fn sync_lsp(&mut self) -> bool {
        // First, and inside this function rather than beside its callers: it takes a format request
        // away when the answer is a program, and every one of the five places that syncs would
        // otherwise have to remember to route first. The early return below is the other half —
        // with no server running, nothing here would ever take the request at all.
        self.route_format();

        let Some(name) = self.active_server.clone() else {
            // A request nothing can answer must not be left sitting here. It would keep until some
            // later buffer started a server and then fire against *that* file — a symbol palette
            // opening by itself over a file the user never asked about, minutes after the key was
            // pressed. Answered at the moment of the keypress instead, which is also the only way
            // the key says anything at all in a project with no server.
            if self.state.pending_request.take().is_some() {
                self.state.hover = Some(format!(
                    "{} — that needs a language server",
                    self.state.server_status
                ));
            }
            return false;
        };
        // Taken before the client is borrowed mutably: the language a file is written in is a
        // question for the config table, and `self` is about to be locked to one server.
        let configured = &self.configured_servers;
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
                        let language = vex_lsp::server_for(&path.to_string_lossy(), configured)
                            .map(|spec| spec.language_id)
                            .unwrap_or_else(|| "plaintext".to_string());
                        match client.did_open(&path, &language, buffer_version.0 as i32, &text) {
                            // Recorded only on success, so a failed open cannot leave a close owing
                            // for a document the server never heard about.
                            Ok(()) => self.opened_document = Some((name.clone(), path.clone())),
                            Err(error) => eprintln!("vex: didOpen failed: {error:#}"),
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
                        // Only reaches here when the server said it formats; `route_format` sends
                        // everything else to a program instead.
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
                        Action::GotoSymbol => {
                            // Opened empty and filled by the reply, the same shape the completion
                            // popup uses: a palette that appears only once the server has answered
                            // is a key that seems to do nothing for a second.
                            self.state.palette = Some(Palette::new(PaletteKind::Symbols));
                            client.document_symbols(&path)
                        }
                        // Re-entered on every keystroke, so the palette is opened only if this is
                        // not already it — otherwise typing would clear the query it is sending.
                        // An empty query is what every server reads as "everything you have".
                        Action::GotoSymbolInProject => {
                            let query = match &self.state.palette {
                                Some(palette) if palette.kind == PaletteKind::ProjectSymbols => {
                                    palette.query.clone()
                                }
                                _ => {
                                    self.state.palette =
                                        Some(Palette::new(PaletteKind::ProjectSymbols));
                                    String::new()
                                }
                            };
                            client.workspace_symbols(&query)
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

        // The rest of whichever suggestion is selected. Sent from here rather than recorded as an
        // `Action` because it is not something a user asks for: it is the editor keeping the row
        // under the cursor filled in, so that accepting it can bring its import.
        if client.state() == vex_lsp::State::Ready {
            if let Some((at, label, raw)) = self
                .state
                .completion
                .as_mut()
                .and_then(vex::completion::Completion::wants_resolving)
            {
                if let Err(error) = client.resolve_completion(at, &label, &raw) {
                    eprintln!("vex: resolve failed: {error:#}");
                }
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
                vex_lsp::ServerEvent::Ready {
                    server_name,
                    formats,
                } => {
                    eprintln!("vex: {server_name} ready (formatting: {formats})");
                    self.state.server_status = server_name;
                    // Keyed by the *program*, not by the name the server calls itself: those differ
                    // — pyright-langserver reports "Pyright" — and `active_server` is the program.
                    self.server_formats.insert(name.clone(), formats);
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
                    // Every file's, kept whatever happens below. The active buffer's are *also*
                    // version-checked, because those are drawn on text whose offsets must still be
                    // valid; a problem in a file that is not open is a path, a line and a message,
                    // and refusing it for a version mismatch would refuse nearly all of them — the
                    // version a server quotes is that file's, and the editor is holding another.
                    self.state.problems.publish(&path, &diagnostics, None);

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
                vex_lsp::ServerEvent::Completion { items, incomplete } => {
                    let caret = self.state.cursor();
                    let text = self.state.document.text().clone();
                    if let Some(completion) = &mut self.state.completion {
                        completion.offer(items, incomplete, &text, caret);
                    }
                }
                vex_lsp::ServerEvent::CompletionResolved { at, label, item } => {
                    if let Some(completion) = &mut self.state.completion {
                        completion.resolved(at, &label, item);
                    }
                }
                vex_lsp::ServerEvent::Symbols { query, symbols } => {
                    let kind = match query {
                        Some(_) => PaletteKind::ProjectSymbols,
                        None => PaletteKind::Symbols,
                    };
                    // A project-wide reply for a query the user has already typed past is
                    // discarded: replies arrive out of order, and a list that flickers back to an
                    // older answer while you are still typing is worse than one that waits.
                    let current = self
                        .state
                        .palette
                        .as_ref()
                        .filter(|palette| palette.kind == kind)
                        .filter(|palette| query.as_deref().is_none_or(|q| q == palette.query));
                    if current.is_none() {
                        continue;
                    }

                    self.symbols = symbols;
                    if self.symbols.is_empty() {
                        // Said, not shown as an empty box: "no symbols" and "the server has not
                        // finished indexing" look identical, and only one of them is worth waiting
                        // through.
                        self.state.hover = Some(format!(
                            "{} found nothing named{}",
                            self.state.server_status,
                            match &query {
                                Some(q) if !q.is_empty() => format!(" like `{q}`"),
                                _ => " here yet".into(),
                            }
                        ));
                    }
                    let filter = kind == PaletteKind::Symbols;
                    let symbols = std::mem::take(&mut self.symbols);
                    if let Some(palette) = &mut self.state.palette {
                        palette.refresh_symbols(&symbols, filter);
                    }
                    self.symbols = symbols;
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
                            state.mutate(|document| {
                                vex::edits::apply_workspace_edit(
                                    document,
                                    path.as_deref(),
                                    &edit,
                                    version,
                                )
                            })
                        },
                    );
                }
                vex_lsp::ServerEvent::Formatting { edits } => {
                    self.apply_server_edit("formatted", |state, version| {
                        state
                            .mutate(|document| {
                                vex::edits::apply_to_document(document, &edits, version)
                            })
                            .map(|count| vex::edits::Applied {
                                in_buffer: count,
                                files: Vec::new(),
                            })
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
        // A format in flight, or its result would sit in the channel until the next keypress —
        // which for `format_on_save` means the save that follows it never happens.
        if self.formatting_version.is_some() {
            return true;
        }
        // The same for git. Without this a revert's reply sat unread until some unrelated key
        // woke the loop, so `]C` in a file with no language server appeared to do nothing at all —
        // and if the key that eventually woke it was a text edit, the revert was then refused as
        // stale and lost outright.
        if self.git.busy() {
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
                let chosen = completion.selection().map(|item| item.offer.clone());
                let anchor = completion.anchor;
                let imports_current = completion.imports_are_current(self.state.document.text());
                self.state.completion = None;

                match chosen {
                    Some(offer) => {
                        self.accept_completion(offer, anchor, imports_current);
                        true
                    }
                    // Nothing selected: let Enter do what Enter does.
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Puts a chosen suggestion into the buffer, with whatever else has to land beside it.
    ///
    /// One `Transaction`, one undo step. Accepting `HashMap` inserts the name *and* adds
    /// `use std::collections::HashMap;` at the top, and undoing that has to take both — an import
    /// left behind by an undone completion is a compile error the user did not write.
    fn accept_completion(
        &mut self,
        offer: vex_lsp::CompletionItem,
        anchor: vex_text::CharOffset,
        imports_current: bool,
    ) {
        let caret = self.state.cursor();
        let accepted = vex::edits::accept_completion(
            self.state.document.text(),
            &offer,
            anchor,
            caret,
            imports_current,
        );

        if accepted.imports_refused {
            self.state.hover = Some(format!(
                "`{}` needs an import, and the file has changed shape since the suggestion was \
                 computed — it was not added. Ask again to get it.",
                offer.label
            ));
        }

        if self
            .state
            .mutate(|document| document.apply(accepted.transaction, vex_buffer::EditKind::Discrete))
            .is_err()
        {
            return;
        }

        // Walk the snippet's tab stops, exactly as an expanded snippet does.
        if let Some(expansion) = accepted.expansion {
            let session = vex::snippet::Session::new(&expansion, accepted.at);
            let stop = session
                .as_ref()
                .and_then(|session| session.current())
                .unwrap_or((
                    vex_text::CharOffset(accepted.at.get() + expansion.text.chars().count()),
                    0,
                ));
            let selection = if stop.1 == 0 {
                Selection::caret(stop.0)
            } else {
                Selection::new(stop.0, vex_text::CharOffset(stop.0.get() + stop.1))
            };
            self.state
                .document
                .set_selections(SelectionSet::single(selection));
            self.snippet = session;
        }

        if accepted.imports > 0 {
            // Said rather than done silently: an edit somewhere the user is not looking is exactly
            // the kind of help that reads as the editor having corrupted the file.
            self.state.server_status = format!("{} · import added", offer.label);
        }
        self.reveal_cursor();
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
        let mut ask_again = false;
        if let Some(completion) = &mut self.state.completion {
            completion.refilter(&text, caret);
            // A truncated list filtered locally forever is how an editor comes to insist a name
            // does not exist: rust-analyzer caps what it sends and sets `isIncomplete`, so the
            // match you are typing towards may simply not be in the list yet. Asking again is what
            // the flag is for — and it is a request, not work on this thread.
            if completion.incomplete && !completion.pending {
                completion.pending = true;
                ask_again = true;
            } else if !completion.pending && completion.items.is_empty() {
                // A session with nothing left to offer is noise.
                self.state.completion = None;
            }
        }
        // Never over a request that is already waiting. `pending_request` is a single slot, and
        // this runs on the way through the key handler *before* the sync that would send one — so
        // a completion refresh landing on the same keystroke as a save would throw away the format
        // that `format_on_save` had just queued, and the file would be written unformatted.
        if ask_again && self.state.pending_request.is_none() {
            self.state.pending_request = Some(Action::RequestCompletion);
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
    /// Writes the file if autosave is on and typing has paused. Returns whether anything happened.
    ///
    /// **A pause, not a timer.** Saving every N seconds while someone is mid-word writes broken
    /// intermediate states to disk, and with `format_on_save` on it would reformat under them as
    /// they type. Waiting for a second of quiet means the file on disk is always a state the author
    /// stopped at.
    ///
    /// Nothing here blocks: it is a clock comparison and, at most, one write of a buffer already in
    /// memory — the same write `ctrl-s` does, on the same thread, with the same cost.
    fn poll_autosave(&mut self) -> bool {
        const QUIET: std::time::Duration = std::time::Duration::from_millis(900);

        if !self.state.autosave || !self.state.document.is_dirty() {
            return false;
        }
        // A buffer with nowhere to go is left alone rather than prompting: autosave is the setting
        // for people who do not want to be asked, and asking them where to put it is the one thing
        // it must not do.
        if self.state.file_path.is_none() {
            return false;
        }
        if self.last_edit.is_none_or(|at| at.elapsed() < QUIET) {
            return false;
        }

        self.save_active();
        true
    }

    fn persist_recovery(&mut self, force: bool) {
        // The active buffer, on the throttle: it is the one being typed into, and the timer exists
        // so a keystroke does not cost a file write.
        if self.state.document.is_dirty() {
            // A scratch buffer has no identity to recover to. Rather than invent one, it is left
            // alone — and that limitation is stated in the plan rather than hidden.
            if let Some(path) = self.state.file_path.clone() {
                let text = self.state.document.text().to_string();
                self.recovery
                    .maybe_write(std::path::Path::new(&path), &text, force);
            }
        }

        if !force {
            return;
        }

        // Every *other* dirty buffer, but only when it matters. This wrote the active document and
        // nothing else, so a file edited and then switched away from was lost entirely in a crash —
        // the one failure the whole recovery mechanism exists to prevent, in the case where the user
        // has done the most work.
        //
        // Only on `force` — quitting, or switching buffers — because a background buffer cannot
        // change without one of those happening first. There is nothing to write on a keystroke.
        for buffer in &self.buffers {
            if !buffer.document.is_dirty() {
                continue;
            }
            let Some(path) = &buffer.file_path else {
                continue;
            };
            let _ = vex::recovery::write_in(
                self.recovery.directory(),
                std::path::Path::new(path),
                &buffer.document.text().to_string(),
            );
        }
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
            "vex: recovered unsaved changes for {path} from {} — the buffer is dirty; {} keeps them",
            found.recovery.display(),
            self.key_for("save")
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
                // way to lose your place. Unless you walked there on purpose with `↵`, in which case
                // putting it back *is* losing your place.
                let origin = prompt.origin;
                let was_search = prompt.is_search();
                let stepped = prompt.stepped;
                self.state.prompt = None;
                if was_search && !stepped {
                    self.state.search_matches.clear();
                    self.state.document.set_primary_caret(origin);
                }
            }
            // `tab` moves between "what to find" and "what to put there". Only replace has two
            // fields, so this is inert everywhere else.
            KeyCode::Tab if prompt.is_replace() => prompt.toggle_field(),

            // `ctrl-↵` replaces every match, as one undoable change. Deliberately the awkward
            // chord: it is the destructive one, and it is not something to arrive at by leaning on
            // the return key.
            KeyCode::Enter if prompt.is_replace() && key.ctrl => self.replace_all(),

            // `↵` in a replace prompt swaps the match under the cursor and moves to the next, which
            // is the loop people actually run: look, replace, look, replace.
            KeyCode::Enter
                if prompt.is_replace() && prompt.focus == vex::prompt::Field::Replacement =>
            {
                self.replace_one()
            }

            // `↵` steps to the next match and leaves the box open, `⇧↵` to the previous one. It used
            // to close the box, so stepping meant knowing that F3 existed and that the thing you had
            // just been using was over — which is why this looked like an editor that could count
            // matches but not visit them.
            KeyCode::Enter if prompt.is_search() => {
                let forward = !key.shift;
                prompt.stepped = true;
                let matches = prompt.matches.clone();
                self.state.search_matches = matches;
                self.step_match(forward);
            }
            KeyCode::Enter => {
                let prompt = self.state.prompt.take().expect("checked above");
                match prompt.kind {
                    PromptKind::Search { .. } | PromptKind::Replace => {
                        // Unreachable: both are handled above. Kept so the match stays total if a
                        // new prompt kind ever wants the old accept-and-close behaviour.
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
                    PromptKind::NewFile => {
                        let relative = prompt.input.trim().to_string();
                        match self.in_project(&relative) {
                            Some(path) if path.exists() => {
                                self.warn(&format!("{relative} already exists"))
                            }
                            Some(path) => {
                                // The parent may not exist — typing `src/new/thing.rs` should work.
                                let made = path
                                    .parent()
                                    .map(std::fs::create_dir_all)
                                    .unwrap_or(Ok(()))
                                    .and_then(|()| std::fs::write(&path, ""));
                                match made {
                                    Ok(()) => {
                                        self.rescan_project();
                                        self.activate(Target::File(path));
                                    }
                                    Err(err) => {
                                        self.warn(&format!("could not create {relative}: {err}"))
                                    }
                                }
                            }
                            None => self.warn("that is outside the project"),
                        }
                    }
                    PromptKind::RenameFile => {
                        let to = prompt.input.trim().to_string();
                        let from = self.tree_target();
                        match (
                            from.as_deref().and_then(|f| self.in_project(f)),
                            self.in_project(&to),
                        ) {
                            (Some(from), Some(to)) if from != to => {
                                if to.exists() {
                                    self.warn(&format!("{} already exists", to.display()));
                                } else {
                                    // Resolved before the rename: once the file has moved,
                                    // `canonical` cannot resolve the old name on *either* side of
                                    // the comparison, and two spellings of it stop matching.
                                    let moving = self.buffers_showing(&from);
                                    let made = to
                                        .parent()
                                        .map(std::fs::create_dir_all)
                                        .unwrap_or(Ok(()))
                                        .and_then(|()| std::fs::rename(&from, &to));
                                    match made {
                                        Ok(()) => {
                                            // Any buffer open on the old path would save it back
                                            // into existence, so they follow the file.
                                            self.rebind_buffers(
                                                &moving,
                                                Some(to.to_string_lossy().to_string()),
                                            );
                                            self.rescan_project();
                                        }
                                        Err(err) => self.warn(&format!("could not rename: {err}")),
                                    }
                                }
                            }
                            (None, _) => self.warn("no file selected"),
                            (_, None) => self.warn("that is outside the project"),
                            _ => {}
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
                    PromptKind::SaveAs => {
                        let path = prompt.input.trim().to_string();
                        if path.is_empty() {
                            self.warn("save cancelled: no path given");
                        } else {
                            // The buffer *becomes* the new file, as everywhere else: you keep
                            // editing what you saved, not the thing you saved it from.
                            let path = absolute_in(&self.state.project_root, &path);
                            self.state.path = relative_crumbs(&path);
                            self.state.file_path = Some(path);
                            self.save_active();
                            self.after_buffer_change();
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                prompt.field_mut().pop();
                self.refresh_prompt();
            }
            // The same query, across every file. This is the other half of "find": you look here,
            // it is not here, and the next thing you want is the project — without retyping it or
            // knowing that a second shortcut exists.
            KeyCode::Char('f') if key.ctrl && key.shift && prompt.is_search() => {
                let query = prompt.input.clone();
                self.state.prompt = None;
                if !query.is_empty() {
                    self.state.results = Some(vex::results::Results::start(
                        &self.state.project_root,
                        &query,
                    ));
                }
            }
            KeyCode::Char(c) if !key.ctrl && !key.alt => {
                prompt.field_mut().push(c);
                self.refresh_prompt();
            }
            _ => {}
        }
        false
    }

    /// Replaces the match at or after the cursor, then moves to the next one.
    fn replace_one(&mut self) {
        let Some(prompt) = &self.state.prompt else {
            return;
        };
        let (matches, replacement) = (prompt.matches.clone(), prompt.replacement.clone());
        if matches.is_empty() {
            return;
        }

        let Some(target) = vex::edits::target_at(&matches, self.state.cursor()) else {
            return;
        };
        let at = target.start;
        match self.state.mutate(|document| {
            vex::edits::replace_matches(document, std::slice::from_ref(&target), &replacement)
        }) {
            Ok(_) => {
                // Where the caret lands: after what was just written, so the next `↵` finds the
                // *next* match rather than re-matching what the replacement happens to contain.
                self.state.document.set_primary_caret(vex_text::CharOffset(
                    at.get() + replacement.chars().count(),
                ));
                self.after_replace();
                self.step_match(true);
            }
            Err(err) => self.state.hover = Some(err.to_string()),
        }
    }

    /// Replaces every match in the buffer, as one undoable change.
    fn replace_all(&mut self) {
        let Some(prompt) = &self.state.prompt else {
            return;
        };
        let (matches, replacement) = (prompt.matches.clone(), prompt.replacement.clone());
        if matches.is_empty() {
            self.state.hover = Some("nothing to replace".into());
            return;
        }

        match self
            .state
            .mutate(|document| vex::edits::replace_matches(document, &matches, &replacement))
        {
            Ok(count) => {
                self.state.hover = Some(format!(
                    "replaced {count} match{}",
                    if count == 1 { "" } else { "es" }
                ));
                self.state.prompt = None;
                self.state.search_matches.clear();
                self.after_replace();
            }
            Err(err) => self.state.hover = Some(err.to_string()),
        }
    }

    /// The bookkeeping a keystroke normally does, which a prompt key skips.
    ///
    /// While a prompt is open the key handler deliberately skips `sync_syntax`, `sync_lsp` and the
    /// recovery write, because until replace existed nothing typed into a prompt could change the
    /// buffer. Replace is the first thing that can, and without this tree-sitter and the language
    /// server would both be describing text that is no longer there.
    fn after_replace(&mut self) {
        self.state.search_matches.clear();
        self.refresh_prompt();
        self.reveal_cursor();
        self.sync_syntax();
        self.sync_lsp();
        self.persist_recovery(false);
        self.last_edit = Some(std::time::Instant::now());
    }

    /// Moves to the next or previous match of the open search, wrapping at the ends.
    fn step_match(&mut self, forward: bool) {
        let matches = self.state.search_matches.clone();
        if matches.is_empty() {
            return;
        }
        // `find_next` compares strictly against the caret and wraps at the ends, so it never returns
        // the match the cursor is already on — which is what makes a second `↵` move a second time.
        let from = self.state.cursor();
        if let Some(target) = vex_buffer::find_next(&matches, from, forward) {
            self.state.document.set_primary_caret(target.start);
            self.reveal_cursor();
        }
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
                    // Against the project root, for the reason `absolute_in` gives: `:w notes.md`
                    // means beside the project, not beside whatever launched the editor.
                    let path = absolute_in(&self.state.project_root, &path);
                    self.state.path = relative_crumbs(&path);
                    self.state.file_path = Some(path);
                }
                self.save_active();
            }
            Command::Quit { force } => {
                if self.state.document.is_dirty() && !force {
                    let message = if self.state.modal {
                        "unsaved changes — :q! to discard, :wq to save".to_string()
                    } else {
                        format!("unsaved changes — {} saves first", self.key_for("save"))
                    };
                    self.warn(&message);
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
///
/// **The one definition.** It existed and was written out by hand at five other sites anyway, which
/// is how the buffer list came to disagree with itself: one of those copies renumbered the whole
/// list on every switch, another never repaired the panes, and nothing tied them together because
/// there was nothing to tie them to.
fn stored_index(buffer: usize, active: usize) -> Option<usize> {
    if buffer == active {
        return None;
    }
    Some(if buffer < active { buffer } else { buffer - 1 })
}

/// What makes two buffers the same buffer.
///
/// The path as the filesystem sees it, not as it was typed. Identity used to be `relative_crumbs` —
/// a *display* path, truncated to its last four segments — so `src/main.rs` and `./src/main.rs`
/// opened twice, with two undo histories and two dirty flags over one file on disk, and two
/// genuinely different files buried five directories deep in different trees collided into one.
///
/// `None` for a scratch buffer, and two scratch buffers are never the same one: nothing on disk
/// backs them, so there is nothing for them to share.
fn buffer_key(file_path: Option<&str>) -> Option<std::path::PathBuf> {
    file_path.map(|path| vex_project::canonical(std::path::Path::new(path)))
}

/// A path typed into one of the editor's own prompts, made absolute against the project root.
///
/// A windowed editor's working directory is wherever the desktop launched it from, and never what
/// someone typing `src/notes.rs` into Save As means. Both `fs::write` and
/// [`vex_project::canonical`] resolve a relative path against that directory, so leaving it
/// relative wrote the file somewhere the user could not find and gave the buffer a key naming a
/// file that did not exist — which then matched nothing on rename or delete.
///
/// Left alone if it is already absolute: "save a copy outside the project" is a real thing to want,
/// and this is not the containment check. That is `in_project`, for the commands that create and
/// destroy files rather than the one that writes the buffer you are looking at.
fn absolute_in(root: &std::path::Path, typed: &str) -> String {
    let path = std::path::Path::new(typed);
    if path.is_absolute() {
        return typed.to_string();
    }
    root.join(path).to_string_lossy().to_string()
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
            Action::CloseBuffer => self.request_close_buffer(self.state.active_buffer),
            Action::NewFile => {
                // Pre-filled with the directory the selection is in, so the common case — a file
                // beside the one you are looking at — is typing a name and pressing enter.
                let mut prompt = vex::prompt::Prompt::new(PromptKind::NewFile, self.state.cursor());
                prompt.input = self.tree_directory();
                self.state.prompt = Some(prompt);
            }
            Action::RenameFile => match self.tree_target() {
                Some(relative) => {
                    let mut prompt =
                        vex::prompt::Prompt::new(PromptKind::RenameFile, self.state.cursor());
                    prompt.input = relative;
                    self.state.prompt = Some(prompt);
                }
                None => self.warn("no file selected"),
            },
            Action::DeleteFile => self.request_delete_file(),
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
                    None => Some(vex::menu::Menu::bar(self.state.modal)),
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
                    let mut settings = vex::settings_ui::Settings::new();
                    settings.servers = self.detect_servers();
                    settings.formatters = self.detect_formatters();
                    self.state.settings = Some(settings);
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
                        // "Jumplist" is Vim's word for it, and this is bound to alt-left and
                        // alt-right in the conventional grammar, where nobody has heard it.
                        self.state.hover = Some(
                            if *forward {
                                "nowhere newer to jump forward to"
                            } else {
                                "nowhere older to jump back to"
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
            Action::RevertHunk => self.request_revert(),
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
        // Through the same reader every other open uses, so the size cap, the binary sniff and the
        // non-UTF-8 refusal apply here too. A session lists whatever was open last time, and a file
        // that has since become a 4GB log is exactly the case `open.rs` exists to bound.
        let contents = match vex::open::read(path) {
            vex::open::Opened::Text(contents) => contents,
            vex::open::Opened::Refused(why) => {
                eprintln!("vex: not restoring {}: {why}", path.display());
                return;
            }
        };

        // Unsaved work from a crash, for *this* file rather than only for whichever one ends up
        // active. Restoring it clean was worse than losing it silently: the first save of that
        // buffer called `recovery.discard`, deleting a copy the user was never shown.
        let recovered = self
            .recovery
            .find(path)
            .filter(|found| found.text != contents);
        let mut document =
            Document::from_str(recovered.as_ref().map_or(&contents, |found| &found.text));
        if recovered.is_some() {
            document.mark_dirty();
            self.state.hover = Some(format!(
                "{} has unsaved changes recovered from a crash",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }

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
        let session = self.session_snapshot();
        vex::session::save(&vex::session::session_dir(), self.index.root(), &session);
    }

    /// Reopens the rest of the last session, behind the file already live.
    ///
    /// A method rather than a block inside `main` so a test can drive the real one. Written as a
    /// loop inside `main`, it was untestable, and the test that would have caught the ordering
    /// defect below had to keep its own copy of the walk — which passes whatever `main` does.
    fn restore_session(&mut self, session: &vex::session::Session) {
        let active = session
            .files
            .get(session.active)
            .map(|file| file.path.clone());

        // Forward. `open_in_background` *appends*, so walking the session backwards restored the
        // other files in the opposite order to the one they were written in — the tab strip came
        // back mirrored, and every buffer number named a different file than it had last time.
        for file in &session.files {
            if Some(&file.path) == active.as_ref() {
                continue;
            }
            self.open_in_background(&file.path, file.line, file.column);
        }

        // Cursor restoration for the active file, which was opened before the app existed.
        if let Some(file) = session.files.get(session.active) {
            self.restore_cursor(file.line, file.column);
        }

        // The strip is built once, from one buffer, before any of this ran. Restoring five files
        // drew five tabs of which four said "untitled" — the drawing code falls back to that name
        // for a tab the list is too short to cover, so nothing failed and it simply looked like the
        // session had not come back.
        self.refresh_buffer_tabs();

        let others = session.files.len().saturating_sub(1);
        if others > 0 {
            eprintln!("vex: restored {others} more from the last session");
        }
    }

    /// What the session file would say. Separate from writing it so the buffer walk — which is
    /// where both of this function's defects lived — can be tested without a window or a disk.
    fn session_snapshot(&self) -> vex::session::Session {
        let mut files = Vec::new();
        let mut active = 0;

        for index in 0..=self.buffers.len() {
            let live = index == self.state.active_buffer;
            let (path, offset, text) = if live {
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

            // *After* that skip, not before it. `active` is an index into `files`, and recording it
            // for a buffer that then turns out to have no path left it pointing at whichever file
            // was written next — so having an untitled buffer in front meant the session came back
            // on the wrong file.
            if live {
                active = files.len();
            }
            let point = text.char_to_point(offset);
            files.push(vex::session::OpenFile {
                path: std::path::PathBuf::from(path),
                line: point.line,
                column: point.column,
            });
        }

        vex::session::Session {
            files,
            active,
            // Only when there is a real window and it is not minimised: saving 0x0 would reopen
            // into nothing.
            window: self
                .window
                .as_ref()
                .map(|window| window.inner_size())
                .filter(|size| size.width > 0 && size.height > 0)
                .map(|size| (size.width, size.height)),
        }
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
            self.state.mutate(|document| document.insert("    "));
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
            .mutate(|document| document.apply(transaction, vex_buffer::EditKind::Discrete))
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
                // Not `save_active`: a save that follows a format must not arm another one, or a
                // formatter that is not idempotent rewrites the file forever.
                if std::mem::take(&mut self.save_after_format) {
                    self.write_active(false);
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

        match self.state.mutate(|document| {
            vex::edits::replace_in_hits(document, path.as_deref(), &hits, pattern, replacement)
        }) {
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

    /// The key that runs a command *in the grammar the user is actually using*.
    ///
    /// For the messages that told people what to press next. Several of them named ex-commands —
    /// `:q!`, `:wq`, `:w <path>` — to everyone, including the people who had turned Vim off and
    /// whose grammar cannot parse the leading `:`. Read from the command table, so a message cannot
    /// drift from the binding it is teaching.
    fn key_for(&self, command: &str) -> &'static str {
        vex_input::commands::by_name(command)
            .map(|command| command.keys_for(self.state.modal))
            .unwrap_or_default()
    }

    /// Keys for the first-run setup. Returns true if the key was consumed — which is always, while it
    /// is showing: it is a modal screen, and letting keys through to a document nobody can see would
    /// be worse than ignoring them.
    /// A click on the welcome screen. Returns whether the wizard took it.
    ///
    /// The wizard's first question is whether you want the conventional editor, and it could only be
    /// answered with the arrow keys — so the person reaching for a mouse to choose *the mouse-shaped
    /// option* found nothing to click. Hovering highlights, one click chooses.
    fn click_welcome(&mut self, x: f32, y: f32) -> bool {
        if self.state.welcome.is_none() {
            return false;
        }
        let bounds = self.window_bounds();
        let scale = self.scale();

        let Some(welcome) = &mut self.state.welcome else {
            return false;
        };
        let geometry = vex::welcome::geometry(welcome, bounds, scale);

        if let Some(index) = geometry
            .options
            .iter()
            .position(|option| vex::layout::contains(*option, x, y))
        {
            welcome.selected = index;
            // On the theme row the highlight *is* the answer, so a click has to preview it too.
            if welcome.step == vex::welcome::Step::Theme {
                welcome.answers.theme = vex_theme::Theme::BUILT_IN[index].0;
            }
            welcome.choose();
            if welcome.is_done() {
                self.finish_welcome();
            }
            return true;
        }

        if geometry
            .next
            .is_some_and(|next| vex::layout::contains(next, x, y))
        {
            // The last row on the import step is "done", which is what this button is.
            welcome.selected = welcome.options().saturating_sub(1);
            welcome.choose();
            if welcome.is_done() {
                self.finish_welcome();
            }
            return true;
        }

        // A click on the background is not an answer, but it must not fall through to the buffer
        // behind the wizard either.
        true
    }

    /// The whole window, as a rectangle.
    fn window_bounds(&self) -> Rect {
        let (width, height) = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.size())
            .unwrap_or((1280, 820));
        Rect::new(0.0, 0.0, width as f32, height as f32)
    }

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

        // ctrl-s writes, as it does everywhere else in the editor. The panel only had `w`, which is
        // Vim's letter for it and is the wrong thing to teach someone who turned Vim off — and who
        // is looking at a panel they reached from a toolbar button.
        if key.ctrl && !key.alt && key.code == KeyCode::Char('s') {
            self.write_settings();
            return true;
        }
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
                vex::settings_ui::Line::Server(_) | vex::settings_ui::Line::Formatter(_) => {
                    self.activate_setting(line)
                }
            },
            KeyCode::Char('h') | KeyCode::Left => match line {
                vex::settings_ui::Line::Option(field) => {
                    settings.dirty = true;
                    self.adjust_setting(field, -1);
                }
                vex::settings_ui::Line::Import(item) => settings.tick(item),
                vex::settings_ui::Line::RunImport
                | vex::settings_ui::Line::LoadTheme
                | vex::settings_ui::Line::Server(_)
                | vex::settings_ui::Line::Formatter(_) => {}
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
            line_numbers: self.state.line_numbers,
            sticky_scroll: self.state.sticky_scroll,
            inlay_hints: self.state.show_inlay_hints,
            autosave: self.state.autosave,
        }
    }

    /// Applies a change immediately. A settings panel whose effect you cannot see is a form.
    fn adjust_setting(&mut self, field: vex::settings_ui::Field, delta: i32) {
        self.change_setting(field, |values| values.adjust(field, delta));
    }

    /// The click path: like `adjust(+1)`, but a number at its limit wraps rather than sticking.
    fn cycle_setting(&mut self, field: vex::settings_ui::Field) {
        self.change_setting(field, |values| values.cycle(field));
    }

    fn change_setting(
        &mut self,
        field: vex::settings_ui::Field,
        change: impl FnOnce(&mut vex::settings_ui::Values),
    ) {
        use vex::settings_ui::Field;

        let mut values = self.setting_values();
        change(&mut values);

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
            Field::LineNumbers => self.state.line_numbers = values.line_numbers,
            Field::StickyScroll => self.state.sticky_scroll = values.sticky_scroll,
            Field::InlayHints => {
                self.state.show_inlay_hints = values.inlay_hints;
                self.state.inlay_hints.clear();
                self.hinted_version = None;
            }
            Field::Autosave => self.state.autosave = values.autosave,
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
        self.write_active(true);
    }

    /// The same, without arming another format.
    ///
    /// Used by the save that *follows* a format. Arming there is a loop with no fixed point when
    /// the formatter is not idempotent: format changes the text, save writes it, `format_on_save`
    /// arms again, format changes it again. A misconfigured prettier or a formatter with a bug is
    /// enough, and the symptom is an editor rewriting your file forever — which is exactly what a
    /// test of the whole path did, twenty seconds of it, before this existed.
    fn write_active(&mut self, then_format: bool) {
        let Some(path) = self.state.file_path.clone() else {
            let message = if self.state.modal {
                "this buffer has no file name — use :w <path>".to_string()
            } else {
                format!(
                    "this buffer has no file name — {} to choose one",
                    self.key_for("save_as")
                )
            };
            self.warn(&message);
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
                //
                // No longer gated on a language server being up: for a plain JavaScript, CSS or
                // Markdown project there is no server and never will be, and `format_on_save`
                // silently doing nothing there was the whole shape of this defect. `route_format`
                // decides which of the two can actually answer.
                if then_format && self.state.format_on_save && self.can_format() {
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
            // Filled when the server answers.
            PaletteKind::CodeActions | PaletteKind::Symbols | PaletteKind::ProjectSymbols => {}
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
        let Some(stored) = stored_index(index, self.state.active_buffer) else {
            return (self.state.path.clone(), true);
        };
        match self.buffers.get(stored) {
            Some(buffer) => (buffer.path.clone(), false),
            None => (self.state.path.clone(), true),
        }
    }

    /// The file buffer `index` is showing, if it has one.
    ///
    /// Kept separate from [`App::buffer_at`], which returns *crumbs* — a display path, and the
    /// thing this used to be compared by.
    fn buffer_file(&self, index: usize) -> Option<&str> {
        match stored_index(index, self.state.active_buffer) {
            None => self.state.file_path.as_deref(),
            Some(stored) => self.buffers.get(stored)?.file_path.as_deref(),
        }
    }

    /// Points the named buffers at a new path, or turns them into scratch buffers — for a file that
    /// has just been renamed or deleted out from under them.
    ///
    /// Takes **indices, resolved before the filesystem operation**, not a path. `canonical` falls
    /// back to returning its input the moment `canonicalize` fails, and it fails as soon as the file
    /// is gone — so afterwards two spellings of one deleted file no longer compare equal, on either
    /// side of the comparison. Taking the key early is not enough; the *buffers'* keys have to be
    /// read early too, and the only way to carry that across the syscall is to carry the answer.
    ///
    /// Every buffer, not only the live one. The file being deleted from the tree is usually *not*
    /// the one on screen, and a stored buffer left pointing at it writes it back into existence the
    /// next time that buffer is saved.
    fn rebind_buffers(&mut self, which: &[usize], now: Option<String>) {
        let rebind = |path: &mut Vec<String>, file: &mut Option<String>| match &now {
            Some(name) => {
                *path = relative_crumbs(name);
                *file = Some(name.clone());
            }
            None => {
                *path = vec!["untitled".to_string()];
                *file = None;
            }
        };

        let mut live = false;
        for index in which {
            match stored_index(*index, self.state.active_buffer) {
                None => {
                    rebind(&mut self.state.path, &mut self.state.file_path);
                    live = true;
                }
                Some(stored) => {
                    if let Some(buffer) = self.buffers.get_mut(stored) {
                        rebind(&mut buffer.path, &mut buffer.file_path);
                    }
                }
            }
        }

        if live {
            // The file on screen is a different file now, and everything keyed to its *language*
            // has to be told: the pairing rules, the syntax worker, the language server. Renaming
            // `a.rs` to `a.py` left it Rust-highlighted with rust-analyzer still attached, and
            // deleting it left a worker parsing a path that was gone.
            self.after_buffer_change();
        } else {
            self.refresh_buffer_tabs();
        }
    }

    /// Whether buffer `index` has unsaved changes.
    fn buffer_is_dirty(&self, index: usize) -> bool {
        match stored_index(index, self.state.active_buffer) {
            None => self.state.document.is_dirty(),
            Some(stored) => self
                .buffers
                .get(stored)
                .is_some_and(|buffer| buffer.document.is_dirty()),
        }
    }

    /// Every buffer showing this file, by global index.
    ///
    /// Usually none or one. More than one is reachable — session restore pushes whatever spelling
    /// the session file held, without going through the already-open check — and a rename that
    /// updated only the first would leave the others writing the old file back.
    fn buffers_showing(&self, path: &std::path::Path) -> Vec<usize> {
        let Some(wanted) = buffer_key(Some(path.to_string_lossy().as_ref())) else {
            return Vec::new();
        };
        (0..=self.buffers.len())
            .filter(|index| {
                buffer_key(self.buffer_file(*index)).as_deref() == Some(wanted.as_path())
            })
            .collect()
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
                // One file's outline is filtered here; the project's is the server's to search,
                // because the editor does not hold a symbol table for a whole project and asking
                // for one up front is the request no server wants.
                PaletteKind::Symbols => palette.refresh_symbols(&self.symbols, true),
                PaletteKind::ProjectSymbols => {
                    self.state.pending_request = Some(Action::GotoSymbolInProject);
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
                // two independent undo histories. Matched on the path the filesystem sees, not on
                // the breadcrumbs: those are truncated to four segments for the title bar, so this
                // both missed the same file spelled differently and confused two different files
                // that happened to end the same way.
                if let Some(index) = self.buffers_showing(&path).first().copied() {
                    self.switch_to(index);
                    return;
                }

                match vex::open::read(&path) {
                    vex::open::Opened::Text(contents) => {
                        // Unsaved work from a crash, if there is any for this file. Only the file
                        // open at startup was ever offered it, so a crash that took three dirty
                        // buffers gave one of them back — and the other two sat in the recovery
                        // directory unmentioned, which is the same as losing them.
                        let recovered = self
                            .recovery
                            .find(&path)
                            .filter(|found| found.text != contents);

                        let mut document = Document::from_str(
                            recovered.as_ref().map_or(&contents, |found| &found.text),
                        );
                        if recovered.is_some() {
                            document.mark_dirty();
                            self.state.hover = Some(format!(
                                "recovered unsaved changes for {} — {} keeps them",
                                path.file_name().unwrap_or_default().to_string_lossy(),
                                self.key_for("save")
                            ));
                        }
                        self.open_document(
                            document,
                            relative_crumbs(&path_string),
                            Some(path_string),
                        );
                    }
                    // On screen, not to a console: a release build on Windows has none, so this
                    // was a click on a file in the tree that appeared to do nothing at all.
                    vex::open::Opened::Refused(why) => {
                        eprintln!("vex: {why}");
                        self.state.hover = Some(why);
                    }
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
            Target::Symbol(index) => self.go_to_symbol(index),
        }
    }

    /// Jumps to a symbol chosen in the palette, opening its file if it is not the one on screen.
    ///
    /// Through the jumplist, like `gd`: going to a symbol is a far-reaching move, and `ctrl-o`
    /// having to bring you back is the difference between exploring a codebase and getting lost in
    /// it.
    fn go_to_symbol(&mut self, index: usize) {
        let Some(symbol) = self.symbols.get(index).cloned() else {
            return;
        };
        let here = self.here();

        // A file-outline symbol carries the path the request was made for, so this is the same code
        // for both scopes; only the project-wide one ever actually changes file.
        let path = symbol.path.to_string_lossy().to_string();
        if !path.is_empty() && Some(&path) != self.state.file_path.as_ref() {
            self.activate(Target::File(symbol.path.clone()));
        }

        // Through `from_lsp`, like every other jump. A server's `character` is a count of UTF-16
        // code units, not of characters — an emoji earlier on the line is two of the first and one
        // of the second — so handing it to `Point::new` as a column lands the caret past the name.
        // That conversion also clamps both line and column, which is what a symbol list outliving
        // the file it describes needs: the server answered for the text it last saw.
        let offset = vex_lsp::from_lsp(self.state.document.text(), symbol.position);
        self.state.document.set_primary_caret(offset);
        self.jumps.push(here);
        self.reveal_cursor();
    }

    /// Applies the code action the user picked.
    fn apply_code_action(&mut self, index: usize) {
        let Some(action) = self.code_actions.get(index).cloned() else {
            return;
        };
        let title = action.title.clone();
        self.apply_server_edit(&title, move |state, version| {
            let path = state.file_path.clone().map(std::path::PathBuf::from);
            state.mutate(|document| {
                vex::edits::apply_workspace_edit(document, path.as_deref(), &action.edit, version)
            })
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

    /// Closes a buffer, refusing once if it has unsaved changes.
    ///
    /// Not a dialog: pressing the same key again closes it, and the message says so. A confirmation
    /// that can be answered by repeating what you already did is faster than one that makes you
    /// find a button, and it still cannot happen by accident — the first press only warns.
    fn request_close_buffer(&mut self, index: usize) {
        let dirty = self.buffer_is_dirty(index);

        if dirty && self.confirm_close != Some(index) {
            self.confirm_close = Some(index);
            let name = self
                .buffer_at(index)
                .0
                .last()
                .cloned()
                .unwrap_or_else(|| "this file".to_string());
            self.state.hover = Some(format!(
                "{name} has unsaved changes — {} saves it, or close again to discard them",
                self.key_for("save")
            ));
            return;
        }

        self.confirm_close = None;
        self.close_buffer(index);
    }

    /// Closes a buffer, keeping the editor on something sensible.
    ///
    /// The last one does not close: an editor with no buffer at all has nowhere to put the cursor,
    /// and "close the file" meaning "quit" is a surprise nobody wants. It becomes an empty scratch
    /// buffer instead, which is what starting vex with no argument already gives you.
    ///
    /// Unsaved changes are not silently discarded — the caller checks first.
    fn close_buffer(&mut self, index: usize) {
        if index > self.buffers.len() {
            return;
        }

        if self.buffers.is_empty() {
            self.state.document = Document::from_str("");
            self.state.path = vec!["untitled".to_string()];
            self.state.file_path = None;
            self.state.scroll = ScrollAnchor::default();
            self.state.highlights = Vec::new();
            self.after_buffer_change();
            self.refresh_buffer_tabs();
            return;
        }

        // Closing the file you are looking at means something else has to become live, so switch
        // away first and then drop what is now a stored buffer. Doing it the other way round would
        // leave the live document pointing at a slot that no longer exists.
        if index == self.state.active_buffer {
            let next = if index == 0 { 1 } else { index - 1 };
            self.switch_to(next);
        }

        // `None` cannot happen here: the branch above already switched away from `index` if it was
        // live. Returning rather than assuming is still cheaper than the alternative, which is
        // removing the wrong buffer.
        let Some(stored) = stored_index(index, self.state.active_buffer) else {
            return;
        };
        if stored < self.buffers.len() {
            self.buffers.remove(stored);
        }

        // The active index counts positions in the whole list, so removing one before it shifts it.
        if index < self.state.active_buffer {
            self.state.active_buffer -= 1;
        }

        // And so does every pane's. Buffers are addressed by position, so a pane above the closed
        // one was left pointing at its neighbour's file — the function that repairs them existed,
        // documented, tested, and was never called from here.
        self.panes.buffer_closed(index);

        self.state.buffer_count = self.buffers.len() + 1;
        self.refresh_buffer_tabs();
    }

    /// The file the tree has selected, relative to the root — or the open file when the tree is
    /// closed, so these commands work from the palette without opening it first.
    fn tree_target(&self) -> Option<String> {
        if let Some(tree) = &self.state.tree {
            if let Some(row) = tree.selected_row() {
                return Some(row.relative.clone());
            }
        }
        let path = self.state.file_path.as_deref()?;
        let relative = std::path::Path::new(path)
            .strip_prefix(&self.state.project_root)
            .unwrap_or(std::path::Path::new(path));
        Some(relative.to_string_lossy().replace('\\', "/"))
    }

    /// The directory a new file should go in: the selected directory, or the selection's parent.
    fn tree_directory(&self) -> String {
        let is_directory = self
            .state
            .tree
            .as_ref()
            .and_then(|tree| tree.selected_row())
            .map(|row| row.is_directory)
            .unwrap_or(false);

        let Some(target) = self.tree_target() else {
            return String::new();
        };
        if is_directory {
            return format!("{target}/");
        }
        match target.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/"),
            None => String::new(),
        }
    }

    /// Resolves a project-relative path, refusing anything that escapes the root.
    ///
    /// A prompt is a text field, and `../../etc/passwd` is a thing a text field can contain. These
    /// commands create, rename and delete real files, so the one place they turn a string into a
    /// path is the place to check it.
    ///
    /// Two checks, because neither alone is enough. `vex_project::within` folds the path lexically,
    /// which is the only thing that works for a file that does not exist yet — and creating one is
    /// the common case here. `canonical` then catches what folding cannot see: a symlink inside the
    /// project pointing out of it. It is asked only where the parent is really on disk, since
    /// otherwise it hands back its input unchanged and answers nothing.
    fn in_project(&self, relative: &str) -> Option<std::path::PathBuf> {
        let path = vex_project::within(&self.state.project_root, relative, cfg!(windows))?;

        if let Some(parent) = path.parent() {
            if parent.exists()
                && !vex_project::canonical(parent)
                    .starts_with(vex_project::canonical(&self.state.project_root))
            {
                return None;
            }
        }
        Some(path)
    }

    /// Deletes the selected file, refusing once first.
    ///
    /// The same shape as closing a dirty buffer, and for a stronger reason: this one cannot be
    /// undone at all. `recovery.rs` does not cover it — it protects unsaved *text*, not files that
    /// used to exist.
    fn request_delete_file(&mut self) {
        let Some(relative) = self.tree_target() else {
            self.warn("no file selected");
            return;
        };
        let Some(path) = self.in_project(&relative) else {
            self.warn("that is outside the project");
            return;
        };
        if path.is_dir() {
            self.warn("vex does not delete folders");
            return;
        }

        if self.confirm_delete.as_deref() != Some(relative.as_str()) {
            self.confirm_delete = Some(relative.clone());
            self.state.hover = Some(format!(
                "delete {relative}? this cannot be undone — choose delete again to confirm"
            ));
            return;
        }
        self.confirm_delete = None;

        // Before the removal, for the same reason rename resolves early: `canonical` cannot resolve
        // a file that is no longer there, and hands back the raw string instead — so afterwards two
        // spellings of it no longer match each other.
        let showing = self.buffers_showing(&path);

        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.state.hover = Some(format!("deleted {relative}"));
                // Any buffer showing it becomes a scratch buffer rather than a window onto a file
                // that no longer exists and would be recreated by the next save. This looked only
                // at the live one, and compared raw strings.
                self.rebind_buffers(&showing, None);
                self.rescan_project();
            }
            Err(err) => self.warn(&format!("could not delete {relative}: {err}")),
        }
    }

    /// Re-walks the project so the tree and the file index agree with the disk.
    fn rescan_project(&mut self) {
        let root = self.state.project_root.clone();
        self.walk = Some(vex_project::FileWalk::start(root));
    }

    /// How many open buffers have unsaved changes.
    fn unsaved_count(&self) -> usize {
        usize::from(self.state.document.is_dirty())
            + self
                .buffers
                .iter()
                .filter(|buffer| buffer.document.is_dirty())
                .count()
    }

    /// What the tab strip draws. Refreshed whenever the buffer list changes.
    fn refresh_buffer_tabs(&mut self) {
        self.state.buffer_tabs = (0..=self.buffers.len())
            .map(|index| {
                let name = self
                    .buffer_at(index)
                    .0
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "untitled".to_string());
                (name, self.buffer_is_dirty(index))
            })
            .collect();
    }

    /// Makes buffer `index` the live one.
    ///
    /// A **rotation**, not a swap. Swapping the live document with the stored one put the buffer
    /// that had been live into the *target's* slot, so the two traded places in the global
    /// numbering: switching from buffer 3 to buffer 1 left 1's file sitting at 3 and 3's at 1. The
    /// tab strip renamed itself under the pointer, and any pane holding an index — every one of
    /// them does — was now showing a different file than the one it was opened on. It happened to
    /// be right for adjacent switches, which is most of them, and wrong for the rest.
    fn switch_to(&mut self, index: usize) {
        let was = self.state.active_buffer;
        if index == was || index > self.buffers.len() {
            return;
        }
        let Some(stored) = stored_index(index, was).filter(|at| *at < self.buffers.len()) else {
            return;
        };

        // Both buffers keep their own undo history and scroll position, which is what makes
        // switching back feel like returning rather than reopening.
        let target = self.buffers.remove(stored);
        let previous = Buffer {
            document: std::mem::replace(&mut self.state.document, target.document),
            path: std::mem::replace(&mut self.state.path, target.path),
            file_path: std::mem::replace(&mut self.state.file_path, target.file_path),
            scroll: std::mem::replace(&mut self.state.scroll, target.scroll),
            // Highlights travel with the buffer so an unfocused pane keeps its colors.
            highlights: std::mem::replace(&mut self.state.highlights, target.highlights),
        };

        // Where the buffer that was live belongs: at its own number, in a list that now skips the
        // one that just became live. This is the whole fix, and it is the one mapping function
        // rather than a second guess at the arithmetic.
        let at = stored_index(was, index).expect("the two indices differ, checked above");
        self.buffers.insert(at, previous);

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
                    revert: None,
                });
                self.diffed_version = Some(version);
            }
        }

        match self.git.poll() {
            Some(vex::git::GitResult::Diff { path, diff })
                if diff.version() == Some(version) && self.is_active_file(&path) =>
            {
                self.state.diff = diff;
                true
            }
            // Stale: the buffer moved on, or the file did, while the diff ran. Dropped, and the
            // newer request that caused the move is already queued.
            Some(vex::git::GitResult::Diff { .. }) => false,
            Some(vex::git::GitResult::Refused(why)) => {
                self.state.hover = Some(why);
                true
            }
            // Counted, not read: it exists so one request always produces one result.
            Some(vex::git::GitResult::Superseded) => false,
            Some(vex::git::GitResult::Revert {
                path,
                version: computed,
                range,
                text,
            }) => self.apply_revert(&path, computed, range, text),
            None => false,
        }
    }

    /// Whether a result computed for `path` is about the buffer now on screen.
    ///
    /// A `Version` is **not** an identity. It counts from `Version(1)` per document, so two files
    /// that have only been opened both hold `Version(1)` — and a result for one passes a version
    /// check against the other. Reverting a hunk in `a.rs` and switching to `b.rs` before `git
    /// show` returned would have replaced lines of `b.rs` with `a.rs`'s committed text, silently
    /// and as one undo group. Nothing about that is caught by comparing numbers.
    /// Compared as written rather than canonicalised: the worker echoes back exactly the path the
    /// request carried, which came from this same field, so they are equal or they are different
    /// files. Canonicalising would touch the filesystem once per poll to learn nothing.
    fn is_active_file(&self, path: &std::path::Path) -> bool {
        self.state
            .file_path
            .as_ref()
            .is_some_and(|open| std::path::Path::new(open) == path)
    }

    /// Asks the git worker what undoing the change under the cursor would put back.
    ///
    /// Not done here: it needs the committed file, a diff against it, and the hunk containing one
    /// line — which is a `git show` plus an LCS, and neither belongs on the thread that draws.
    fn request_revert(&mut self) {
        let Some(path) = self.state.file_path.clone() else {
            self.warn("a scratch buffer has nothing committed to go back to");
            return;
        };
        if self.state.diff.is_empty() {
            self.state.hover = Some("nothing has changed since the last commit".into());
            return;
        }
        self.git.request(vex::git::DiffRequest {
            path: std::path::PathBuf::from(path),
            text: self.state.document.text().to_string(),
            version: self.state.document.version(),
            revert: Some(self.state.cursor_point().line),
        });
        // The worker's drain loop stops at a revert, so a diff queued behind this one is dropped
        // rather than computed. Forgetting what was asked for makes the next wake queue a fresh
        // diff, which then runs straight after the revert — otherwise a revert that turns out to
        // be refused leaves the gutter describing text that has already moved.
        self.diffed_version = None;
    }

    /// Puts a hunk back to how it was committed.
    fn apply_revert(
        &mut self,
        path: &std::path::Path,
        computed: vex_text::Version,
        range: std::ops::Range<usize>,
        text: String,
    ) -> bool {
        // The one rule again: worked out against a version the user has since typed past, so the
        // line numbers no longer mean what they meant — or worked out for a *different file*, if
        // the buffer was switched while `git show` ran. Refused rather than applied to whatever is
        // at those lines now, which for a destructive action is the difference between doing
        // nothing and destroying the wrong thing.
        if !self.is_active_file(path) {
            self.state.hover =
                Some("the buffer changed while that was being worked out — ask again".into());
            return true;
        }
        if self.state.document.version() != computed {
            self.state.hover =
                Some("the file changed while that was being worked out — ask again".into());
            return true;
        }

        let document = self.state.document.text();
        let last = document.last_line();
        let start = document.point_to_char(vex_text::Point::new(range.start.min(last), 0));
        let end = if range.end > last {
            document.len_chars()
        } else {
            document.point_to_char(vex_text::Point::new(range.end, 0))
        };

        let edit = vex_text::Edit::replace(start..end, text);
        match self.state.mutate(|document| {
            document.apply(
                vex_text::Transaction::new(vec![edit]),
                vex_buffer::EditKind::Discrete,
            )
        }) {
            Ok(_) => {
                // One undo step, so a revert taken by mistake is one press of `u` away. That
                // matters more here than anywhere else: this is the only action in the editor whose
                // whole purpose is to throw work away.
                self.state.hover = Some(format!(
                    "put {} back to the committed version — {} undoes it",
                    if range.len() == 1 {
                        "1 line".to_string()
                    } else {
                        format!("{} lines", range.len())
                    },
                    self.key_for("undo")
                ));
                self.diffed_version = None;
                self.sync_syntax();
                self.reveal_cursor();
            }
            Err(error) => self.state.hover = Some(format!("could not undo that change: {error}")),
        }
        true
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

        // The wizard owns the whole window while it is up, so it gets the click first.
        if !extend && self.click_welcome(x, y) {
            return;
        }

        // An open menu takes the click, wherever it lands: on a row it runs it, on a title it opens
        // that menu, anywhere else it closes — which is what clicking away from a menu means
        // everywhere, and the only way out for someone who opened one by accident.
        if self.state.menu.is_some() && !extend {
            self.click_menu(x, y, scale);
            return;
        }

        // The toolbar, before anything else in the tab strip: it is the mouse's way to the commands
        // that otherwise need a shortcut, and it is the whole reason someone who chose the
        // conventional editor can use this without being told anything.
        if !extend && self.click_toolbar(x, y) {
            return;
        }

        if !extend && self.click_tabs(layout.tabs, x, y, scale) {
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
        let offset = self.offset_at(rects.code, x, y, scale);

        // Alt-drag draws a rectangle. Held before the button goes down it means "another cursor";
        // held while the pointer moves it means "a cursor on every line between these two columns",
        // which is the same idea at a different scale and is why one modifier serves both.
        //
        // The cursor the opening alt-click added is simply replaced — the whole set is rebuilt from
        // the rectangle each time the pointer moves, so there is nothing left over from the press.
        if extend {
            if let Some(origin) = self.block_origin {
                let text = self.state.document.text();
                let head = text.char_to_point(offset);
                self.state
                    .document
                    .set_selections(vex_text::SelectionSet::column(text, origin, head));
                self.sync_syntax();
                return;
            }
        } else if self.modifiers.alt_key() {
            self.block_origin = Some(self.state.document.text().char_to_point(offset));
        }

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

    /// The buffer offset a point inside a pane's code area names.
    ///
    /// One definition, because two would drift and the symptom — a click landing a line off — gets
    /// blamed on the display map rather than on the copy that drifted.
    fn offset_at(&self, code: Rect, x: f32, y: f32, scale: f32) -> vex_text::CharOffset {
        // The same arithmetic that placed the glyph the pointer is over. It used to be a second
        // copy, and the two already disagreed about the left pad — drawing used 12, this used 8 —
        // so a click landed most of a character out before any of the horizontal work existed.
        let grid = self.code_grid(code, scale);
        let height = self
            .state
            .visible_rows(grid.code.height, grid.row_height())
            .max(1);
        self.state.display.offset_at_display_point(
            self.state.document.text(),
            self.state.scroll,
            height,
            vex_text::DisplayPoint {
                // Rows and columns clamped: a click below the last line means the last line, not
                // nothing.
                row: grid.row_at(y),
                column: grid.column_at(x),
            },
        )
    }

    /// The grid for a pane's code area, at the focused buffer's scroll position.
    ///
    /// Sticky scroll takes its strip off the top first, through the same `CodeArea` the drawing
    /// side splits with. Without it the mouse measured from the top of the whole pane while the
    /// text was drawn a row lower per pinned head, so a click landed up to three lines below the
    /// line under the pointer — and worsened as you nested deeper, which read as intermittent.
    fn code_grid(&self, code: Rect, scale: f32) -> vex::layout::CodeGrid {
        let row_height = self.state.typography.row_height() * scale;
        let area = vex::layout::CodeArea::new(
            code,
            self.state.sticky_heads(
                self.state.document.text(),
                self.state.scroll.line,
                // Only the focused pane pins anything, and this grid is only ever built for it.
                true,
            ),
            row_height,
        );
        vex::layout::CodeGrid::new(
            area.code,
            self.state.typography.code_size * 0.6 * scale,
            row_height,
            if self.state.display.mode == WrapMode::None {
                self.state.scroll.column
            } else {
                0
            },
            scale,
        )
    }

    /// The focused pane's grid, for the wheel and the horizontal reveal.
    fn focused_grid(&self) -> vex::layout::CodeGrid {
        let layout = self.layout();
        let scale = self.scale();
        let rects = layout.pane(
            self.panes.active(),
            self.panes.len(),
            self.panes.layout == chrome::PaneLayout::Vertical,
            scale,
        );
        self.code_grid(rects.code, scale)
    }

    /// Whether the pointer sits inside a selection in the focused pane.
    ///
    /// The question a right-click has to ask before it moves the caret. Only the focused pane: a
    /// selection in another one is not what a menu opened here would act on.
    fn pointer_is_in_a_selection(&self) -> bool {
        let (x, y) = self.pointer;
        let scale = self.scale();
        let layout = self.layout();
        let count = self.panes.len();
        let vertical = self.panes.layout == chrome::PaneLayout::Vertical;

        let Some(index) = layout.pane_at(x, y, count, vertical, scale) else {
            return false;
        };
        if index != self.panes.active() {
            return false;
        }

        let offset = self.offset_at(layout.pane(index, count, vertical, scale).code, x, y, scale);
        selection_contains(self.state.document.selections(), offset)
    }

    /// Whether the pointer is over the tab strip — a toolbar button, or a tab.
    ///
    /// Both light up under the pointer, so both need the frame redrawn when it arrives or leaves.
    fn over_toolbar(&self) -> bool {
        let (x, y) = self.pointer;
        let layout = self.layout();
        vex::layout::contains(layout.tabs, x, y)
    }

    /// A click on a tab, or on a tab's close button.
    ///
    /// The close button is inside the tab, so it is tested first — otherwise closing a file would
    /// switch to it instead, which is the more annoying of the two wrong answers.
    fn click_tabs(&mut self, strip: Rect, x: f32, y: f32, scale: f32) -> bool {
        let tabs = vex::layout::tabs(
            strip,
            self.state.buffer_count,
            self.state.active_buffer,
            scale,
        );

        for tab in tabs {
            if vex::layout::contains(tab.close, x, y) {
                self.request_close_buffer(tab.index);
                return true;
            }
            if vex::layout::contains(tab.rect, x, y) {
                self.switch_to(tab.index);
                return true;
            }
        }
        false
    }

    /// A click on a toolbar button, if that is where it landed.
    fn click_toolbar(&mut self, x: f32, y: f32) -> bool {
        let layout = self.layout();
        let hit = vex::layout::toolbar_buttons(layout.tabs, self.scale())
            .into_iter()
            .find(|(_, box_)| vex::layout::contains(*box_, x, y))
            .map(|(button, _)| button.command);

        let Some(command) = hit else {
            return false;
        };
        // Through the action, exactly as the key does: the button cannot drift from the shortcut it
        // is teaching, because they are the same call.
        if let Some(action) = Action::from_name(command) {
            let _ = self.run(action);
        }
        true
    }

    /// A click while a menu is open.
    fn click_menu(&mut self, x: f32, y: f32, scale: f32) {
        let bounds = self.window_bounds();
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

        // The card is padded, and the pointer sits on its top-left corner the instant a context
        // menu opens — inside the card, above the first row. Treating that strip as "clicked away"
        // meant a right-click followed by a left-click without moving the mouse dismissed the menu
        // and did nothing, which is the first thing anybody tries.
        if chosen.is_none() && vex::layout::contains(geometry.card, x, y) {
            return;
        }

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
        if let Some(tree) = &self.state.tree {
            // The same rectangles the drawing code uses, from the same function. This kept its own
            // copy of the walk, and the copies had already drifted: drawing left a pad's width free
            // at the bottom of the panel and this did not, so the two asked for different numbers
            // of visible rows. Whenever the tree was longer than the panel that is a different
            // *window* of rows, and clicking one file opened another.
            let panel = vex::tree::panel(sidebar, scale, tree);
            let Some(index) = panel.row_at(y) else {
                return;
            };
            let target = panel.rows[index].0.relative.clone();
            let Some(tree) = &mut self.state.tree else {
                return;
            };

            // Selecting by path rather than by index, because the window may be scrolled.
            tree.reveal(&target);
            if let Some(path) = tree.activate() {
                self.activate(Target::File(path));
            }
            return;
        }

        // The problems list. Choosing a row goes there — the panel exists to decide where to go
        // next, and one you can only read is half a panel.
        if self.state.show_diagnostics {
            let pad = space::LG * scale;
            let line = 20.0 * scale;
            let top = sidebar.y + pad + line * 1.4 + line * 1.6;

            let here = self.state.file_path.clone();
            let problems = self
                .state
                .problems
                .ordered(here.as_deref().map(std::path::Path::new));

            // The same walk the drawing does: a heading each time the file changes, then two lines
            // per problem. Kept in step by shape rather than by a shared function because the panel
            // draws no rectangles the mouse needs beyond these rows.
            let mut cursor = top;
            let mut last: Option<&std::path::Path> = None;
            for problem in &problems {
                if cursor + line * 2.0 > sidebar.bottom() - pad {
                    break;
                }
                if last != Some(problem.path.as_path()) {
                    last = Some(problem.path.as_path());
                    cursor += line * 1.2;
                    if cursor + line * 2.0 > sidebar.bottom() - pad {
                        break;
                    }
                }
                if y >= cursor && y < cursor + line * 2.0 {
                    let path = problem.path.clone();
                    let (target_line, target_column) = (problem.line, problem.column);
                    self.activate(Target::File(path));
                    self.restore_cursor(target_line, target_column);
                    self.reveal_cursor();
                    return;
                }
                cursor += line * 2.0;
            }
            return;
        }

        if let Some(settings) = &self.state.settings {
            // The same rectangles the drawing code uses, from the same function. This walked the
            // list itself once, which was survivable until the panel started scrolling and the two
            // walks stopped agreeing — a click then selected the row that used to be there.
            let panel = vex::settings_ui::panel(sidebar, scale, settings);
            let Some(hit) = panel
                .rows
                .iter()
                .find(|row| y >= row.rect.y && y < row.rect.bottom())
            else {
                return;
            };
            let (index, entry) = (hit.index, hit.line);

            if let Some(settings) = &mut self.state.settings {
                settings.selected = index;
            }
            // Selecting is not enough. A click that highlighted a row and then waited to be told
            // `l` is a panel a mouse user cannot operate at all — which is what this was. A click
            // *is* the change: switches flip, choices advance, and the action rows run.
            self.activate_setting(entry);
        }
    }

    /// What a click on a settings row does. The same paths `l` and `enter` take.
    fn activate_setting(&mut self, line: vex::settings_ui::Line) {
        match line {
            vex::settings_ui::Line::Option(field) => {
                if let Some(settings) = &mut self.state.settings {
                    settings.dirty = true;
                }
                // Forward, and the numeric fields wrap at their limit, so a mouse can walk a font
                // size all the way round rather than sticking at 32 with no way back.
                self.cycle_setting(field);
            }
            vex::settings_ui::Line::Import(item) => {
                if let Some(settings) = &mut self.state.settings {
                    settings.tick(item);
                }
            }
            vex::settings_ui::Line::RunImport => self.run_import(),
            vex::settings_ui::Line::LoadTheme => self.prompt_for_theme_file(),
            // A report, not a control — so pressing it does the one useful thing a 292-pixel row
            // cannot: shows the whole line, which for a language server is a path that never fits.
            vex::settings_ui::Line::Server(at) | vex::settings_ui::Line::Formatter(at) => {
                let formatter = matches!(line, vex::settings_ui::Line::Formatter(_));
                if let Some(row) = self.state.settings.as_ref().and_then(|settings| {
                    if formatter {
                        settings.formatters.get(at)
                    } else {
                        settings.servers.get(at)
                    }
                }) {
                    self.state.hover = Some(format!(
                        "{} — {} · {}",
                        row.language, row.program, row.status
                    ));
                }
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

        // Servers already running are left alone — a reload is not a reason to throw away minutes
        // of rust-analyzer's indexing. The new table takes effect the next time one is started,
        // and the panel's report is refreshed so it does not describe the file as it used to be.
        self.configured_servers = settings.servers;
        self.configured_formatters = settings.formatters;
        self.missing_servers.clear();
        if self.state.settings.is_some() {
            let servers = self.detect_servers();
            let formatters = self.detect_formatters();
            if let Some(panel) = &mut self.state.settings {
                panel.servers = servers;
                panel.formatters = formatters;
            }
        }

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
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                self.state.hover = Some(if self.state.modal {
                    format!(
                        "{name} changed on disk, and this buffer has unsaved changes — \
                         :e to discard yours, :w to overwrite theirs"
                    )
                } else {
                    format!(
                        "{name} changed on disk, and this buffer has unsaved changes — {} \
                         overwrites theirs",
                        self.key_for("save")
                    )
                });
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
        // Recorded as you open it rather than on the way out, so it survives a crash and so the next
        // launch lands here — which is the whole point of remembering it.
        vex::session::remember_project(&vex::session::session_dir(), &root);
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

        // The names in the tab strip. Here rather than per frame: the list changes when a buffer
        // opens, closes or is switched to, and nothing else moves it.
        self.refresh_buffer_tabs();

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
        // Minimised: there is nothing to draw into, and building a frame for it is the same waste
        // as submitting one.
        if !renderer.is_visible() {
            return;
        }

        // The clock starts here, not inside `render`: building the frame is the half of the work
        // this project has optimised hardest, and it was not in the number the statusline showed.
        renderer.timer.begin();

        // The active tab's dot follows the document's dirty flag, which changes on any keystroke.
        // One bool, rather than rebuilding the whole strip: the names only change when the buffer
        // list does.
        let dirty = self.state.document.is_dirty();
        let active = self.state.active_buffer;
        if let Some(tab) = self.state.buffer_tabs.get_mut(active) {
            tab.1 = dirty;
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_right_click_inside_a_selection_is_about_that_selection() {
        // Opening the context menu moved the caret unconditionally, which collapsed the selection
        // the menu was about. `Target::Selection` on a bare caret is one character, so `copy` put a
        // single character on the clipboard and `cut` deleted one — both indistinguishable from a
        // menu row that does nothing, which is how it was reported.
        use vex_text::{CharOffset, Selection, SelectionSet};

        let selected = SelectionSet::single(Selection::new(CharOffset(10), CharOffset(20)));
        assert!(selection_contains(&selected, CharOffset(15)), "inside");
        assert!(
            selection_contains(&selected, CharOffset(10)),
            "at the start"
        );
        assert!(
            !selection_contains(&selected, CharOffset(20)),
            "the end is exclusive"
        );
        assert!(!selection_contains(&selected, CharOffset(4)), "before it");

        let caret = SelectionSet::single(Selection::caret(CharOffset(10)));
        assert!(
            !selection_contains(&caret, CharOffset(10)),
            "a caret is not a selection, so a right-click on one still moves it"
        );
    }

    #[test]
    fn breadcrumbs_split_on_either_separator() {
        // Windows hands the editor backslashes. Splitting on `/` alone made the whole path one crumb,
        // so the title bar showed `crates\vex\src\main.rs` where Linux showed four breadcrumbs — a
        // defect no test could fail on and only a screenshot could show.
        assert_eq!(
            relative_crumbs(r"crates\vex\src\main.rs"),
            vec!["crates", "vex", "src", "main.rs"]
        );
        assert_eq!(
            relative_crumbs("crates/vex/src/main.rs"),
            vec!["crates", "vex", "src", "main.rs"]
        );
    }

    #[test]
    fn a_long_path_keeps_only_its_last_few_segments() {
        // The title bar is one line. A path from the filesystem root would push the file name off it,
        // which is the one part that has to be visible.
        let crumbs = relative_crumbs("/home/someone/code/project/crates/vex/src/main.rs");
        assert_eq!(crumbs.len(), 4);
        assert_eq!(crumbs.last().map(String::as_str), Some("main.rs"));
    }

    #[test]
    fn a_leading_dot_slash_is_not_a_crumb() {
        assert_eq!(relative_crumbs("./src/main.rs"), vec!["src", "main.rs"]);
        assert_eq!(relative_crumbs(r".\src\main.rs"), vec!["src", "main.rs"]);
    }
}

/// Driving the editor's **wiring** — the paths between the pure functions.
///
/// Every one of these exists because an adversarial review found a defect there that no test could
/// have caught. The pattern was consistent and worth stating: the pure pieces were tested well
/// (`format::run`, `format::narrow`, `git::revert`, `edits::accept_completion` all had tests that
/// passed), and the code that *calls* them had none — so a formatter that never applied its result
/// and a revert that could overwrite a different file both shipped green.
///
/// No window, no renderer, no event loop. `App::new` builds the same editor `run` does; these drive
/// it by calling the methods `window_event` and `about_to_wait` would.
#[cfg(test)]
mod wiring_tests {
    use super::*;

    /// A directory of this test's own, so no two can interfere.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-wiring-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor with one file open, rooted at `dir`.
    fn editor(dir: &std::path::Path, name: &str, contents: &str) -> App {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();

        let text = Text::from_str(contents);
        let file = path.to_string_lossy().to_string();
        let (state, _) = new_state(
            text,
            vec![name.to_string()],
            Some(file),
            Mode::Insert,
            None,
            dir.to_path_buf(),
        );
        App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &vex::config::Settings::default(),
        )
    }

    /// Runs the editor's idle work until `done`, or gives up. Never spins forever: a wiring defect
    /// that hangs must fail the test rather than the suite.
    fn settle(app: &mut App, mut done: impl FnMut(&App) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            app.route_format();
            app.poll_formatter();
            app.sync_git();
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// A program that behaves like a formatter: reads stdin, writes stdout.
    fn tidy_formatter() -> Option<vex::format::Configured> {
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "more".to_string()])
        } else {
            ("cat", Vec::new())
        };
        vex_lsp::resolve(program)?;
        vex::format::configure("text", program, &args, &["txt".to_string()]).ok()
    }

    #[test]
    fn an_external_format_actually_reaches_the_buffer() {
        // The defect this exists for: `poll_formatter` cleared the version it was about to compare
        // against, so the guard read `None != Some(v)` and every format was refused as stale on a
        // buffer nobody had touched. The whole feature was inert and four unit tests passed.
        let Some(formatter) = tidy_formatter() else {
            eprintln!("skipping: no pass-through program on this machine");
            return;
        };
        let dir = scratch("format-applies");
        let mut app = editor(&dir, "notes.txt", "one\ntwo\n");
        app.configured_formatters = vec![formatter];

        // A pass-through leaves the text alone or rewrites its line endings depending on the
        // platform, so the status is "formatted" or "already formatted" — both mean the reply was
        // *used*. The failure being guarded against is the third answer, "refused", which is what
        // every format reported before the fix.
        app.state.pending_request = Some(Action::Format);
        assert!(
            settle(&mut app, |app| app
                .state
                .server_status
                .starts_with("formatted")
                || app.state.server_status == "already formatted"),
            "the format was never applied — status was {:?}",
            app.state.server_status
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_external_format_that_changes_the_text_applies_it() {
        // The other half: a formatter whose output differs has to reach the document.
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "echo tidied".to_string()])
        } else {
            ("sh", vec!["-c".to_string(), "echo tidied".to_string()])
        };
        if vex_lsp::resolve(program).is_none() {
            eprintln!("skipping: no {program} on this machine");
            return;
        }
        let formatter =
            vex::format::configure("text", program, &args, &["txt".to_string()]).unwrap();

        let dir = scratch("format-changes");
        let mut app = editor(&dir, "notes.txt", "untidy\n");
        app.configured_formatters = vec![formatter];

        app.state.pending_request = Some(Action::Format);
        assert!(
            settle(&mut app, |app| app.state.server_status == "formatted"),
            "status was {:?}",
            app.state.server_status
        );
        assert!(
            app.state.document.text().to_string().contains("tidied"),
            "the formatter's output is in the buffer"
        );
        // One undo step: a format is one intent however many lines it rewrote.
        app.state.mutate(|document| document.undo());
        assert_eq!(app.state.document.text().to_string(), "untidy\n");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_format_that_cannot_run_does_not_leave_a_save_armed() {
        // The defect: the "no formatter on this machine" early return kept `save_after_format`,
        // and the next server edit anywhere in the editor took it and wrote *that* buffer to disk —
        // a different file, with whatever unsaved work was in it.
        let dir = scratch("format-no-program");
        let mut app = editor(&dir, "notes.txt", "one\n");
        app.configured_formatters = vec![
            vex::format::configure("text", "vex-no-such-formatter", &[], &["txt".to_string()])
                .unwrap(),
        ];

        app.save_after_format = true;
        app.state.pending_request = Some(Action::Format);
        app.route_format();

        assert!(
            !app.save_after_format,
            "a save that can never happen must not stay armed"
        );
        assert!(
            app.state.hover.is_some(),
            "and the user is told why nothing happened"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn formatting_an_already_tidy_file_does_not_loop() {
        // The defect: the "already formatted" branch fell through to the save-after-format
        // handshake, which re-armed `format_on_save`, which formatted, which found nothing, which
        // saved. No fixed point.
        let Some(formatter) = tidy_formatter() else {
            eprintln!("skipping: no pass-through program on this machine");
            return;
        };
        let dir = scratch("format-fixed-point");
        let mut app = editor(&dir, "notes.txt", "one\ntwo\n");
        app.configured_formatters = vec![formatter];
        app.state.format_on_save = true;

        // Settled until nothing is outstanding. Termination is the whole assertion: a save that
        // re-arms a format that finds nothing and saves again has no fixed point, and the deadline
        // inside `settle` is what a genuine loop trips.
        app.save_after_format = true;
        app.state.pending_request = Some(Action::Format);
        assert!(
            settle(&mut app, |app| !app.save_after_format
                && app.state.pending_request.is_none()
                && !app.services_busy()),
            "save -> format -> save never reached a fixed point (status {:?})",
            app.state.server_status
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_request_nothing_can_answer_is_answered_rather_than_kept() {
        // The defect: with no language server, `sync_lsp` returned before taking the request, so
        // ctrl-shift-o did nothing and said nothing — and the request stayed, firing unbidden
        // against whichever file next started a server.
        let dir = scratch("no-server");
        let mut app = editor(&dir, "notes.txt", "one\n");

        app.state.pending_request = Some(Action::GotoSymbol);
        app.sync_lsp();

        assert!(
            app.state.pending_request.is_none(),
            "a request that cannot be answered must not outlive the keypress"
        );
        assert!(
            app.state.hover.is_some(),
            "and the key has to say something rather than appearing broken"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_git_result_for_another_file_is_refused() {
        // The worst defect the review found. A `Version` counts from 1 per document, so two files
        // that have only been opened both hold `Version(1)` — reverting a hunk in one and switching
        // to the other before `git show` returned passed the version check and replaced lines of
        // the second file with the first one's committed text.
        let dir = scratch("git-identity");
        let mut app = editor(&dir, "a.txt", "one\ntwo\n");
        let before = app.state.document.text().to_string();

        // A result that is perfectly valid, for a file that is not the one on screen.
        let version = app.state.document.version();
        let applied = app.apply_revert(
            &dir.join("somewhere-else.txt"),
            version,
            0..1,
            "REPLACED\n".to_string(),
        );

        assert!(applied, "it was handled");
        assert_eq!(
            app.state.document.text().to_string(),
            before,
            "and it did not touch the buffer it was not about"
        );
        assert!(app.state.hover.is_some(), "with a sentence saying so");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_revert_puts_the_committed_text_back_in_the_real_editor() {
        // The whole path, against a real repository: key -> worker -> `git show` -> diff -> hunk ->
        // one Discrete edit in the buffer.
        if vex::process::command("git")
            .arg("--version")
            .output()
            .map(|out| !out.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: no git on this machine");
            return;
        }
        let dir = scratch("git-revert");
        let git = |args: &[&str]| {
            vex::process::command("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        std::fs::write(dir.join("notes.txt"), "one\ntwo\nthree\n").unwrap();
        if git(&["init", "-q"]).is_none()
            || git(&["config", "user.email", "vex@example.invalid"]).is_none()
            || git(&["config", "user.name", "vex"]).is_none()
            || git(&["add", "."]).is_none()
            || git(&["commit", "-q", "-m", "first"]).is_none()
        {
            eprintln!("skipping: could not build a repository here");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }

        let mut app = editor(&dir, "notes.txt", "one\nCHANGED\nthree\n");
        app.state.document.set_primary_caret(CharOffset(4));

        // The gutter first, which is also what `request_revert` checks before asking.
        assert!(
            settle(&mut app, |app| !app.state.diff.is_empty()),
            "the diff never arrived"
        );

        app.request_revert();
        assert!(
            settle(&mut app, |app| app.state.document.text().to_string()
                == "one\ntwo\nthree\n"),
            "the revert never landed — buffer is {:?}",
            app.state.document.text().to_string()
        );

        // One undo step, because this is the one action whose purpose is to throw work away.
        app.state.mutate(|document| document.undo());
        assert_eq!(
            app.state.document.text().to_string(),
            "one\nCHANGED\nthree\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_editor_does_not_sleep_while_a_worker_still_owes_an_answer() {
        // The defect: `services_busy` had no git clause, so a revert's reply sat unread in the
        // channel until some unrelated keypress woke the loop — and if that keypress was an edit,
        // the revert was then refused as stale and lost outright.
        let dir = scratch("busy");
        let mut app = editor(&dir, "notes.txt", "one\n");
        assert!(!app.services_busy(), "nothing outstanding to start with");

        let version = app.state.document.version();
        app.git.request(vex::git::DiffRequest {
            path: dir.join("notes.txt"),
            text: "one\n".to_string(),
            version,
            revert: None,
        });
        assert!(
            app.services_busy(),
            "a git request in flight must keep the loop awake"
        );

        assert!(settle(&mut app, |app| !app.services_busy()));

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::*;

    /// A directory of this test's own, so no two can interfere.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-buffers-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor rooted at `dir` with `names` open, in order, the last one live.
    fn editor(dir: &std::path::Path, names: &[&str]) -> App {
        for name in names {
            std::fs::write(dir.join(name), format!("contents of {name}\n")).unwrap();
        }
        let first = dir.join(names[0]);
        let (state, _) = new_state(
            Text::from_str(&format!("contents of {}\n", names[0])),
            vec![names[0].to_string()],
            Some(first.to_string_lossy().to_string()),
            Mode::Insert,
            None,
            dir.to_path_buf(),
        );
        let mut app = App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &vex::config::Settings::default(),
        );
        for name in &names[1..] {
            app.activate(Target::File(dir.join(name)));
        }
        app
    }

    /// What each buffer number is showing, by file name — the global numbering, the one the tabs
    /// and the panes both count in.
    fn names(app: &App) -> Vec<String> {
        (0..=app.buffers.len())
            .map(|index| {
                app.buffer_file(index)
                    .and_then(|path| {
                        std::path::Path::new(path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                    })
                    .unwrap_or_else(|| "untitled".into())
            })
            .collect()
    }

    #[test]
    fn switching_buffers_does_not_renumber_them() {
        // It swapped the live document with the stored one, which put the buffer that had been
        // live into the *target's* slot: switching from 3 to 1 left 1's file at 3 and 3's at 1. The
        // tab strip renamed itself under the pointer, and every pane holds one of these numbers. It
        // happened to be right for adjacent switches, which is most of them.
        let dir = scratch("renumber");
        let mut app = editor(&dir, &["a.rs", "b.rs", "c.rs", "d.rs"]);

        let before = names(&app);
        assert_eq!(before, ["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert_eq!(app.state.active_buffer, 3);

        for target in [1usize, 3, 0, 2, 0, 3, 1] {
            app.switch_to(target);
            assert_eq!(
                app.state.active_buffer, target,
                "switching to {target} did not make it live"
            );
            assert_eq!(
                names(&app),
                before,
                "switching to {target} moved the other buffers"
            );
        }
    }

    #[test]
    fn closing_a_buffer_moves_the_panes_watching_the_ones_above_it() {
        // Buffers are addressed by position, so closing one shifts every index above it.
        // `Panes::buffer_closed` exists, is documented, is tested in its own module — and was never
        // called. A split pane showing buffer 3 kept showing "3" after buffer 1 closed, which is a
        // different file.
        let dir = scratch("panes");
        let mut app = editor(&dir, &["a.rs", "b.rs", "c.rs", "d.rs"]);

        app.panes.split(chrome::PaneLayout::Vertical);
        app.switch_to(0);

        // The unfocused pane goes on watching d.rs while this one shows a.rs, which is what a split
        // is for. The focused pane follows the active buffer on its own, so it cannot show this.
        app.panes.cycle(true);
        app.panes.focused_mut().buffer = 3;
        app.panes.cycle(true);
        assert_eq!(names(&app)[3], "d.rs");

        app.close_buffer(1);

        let watching: Vec<Option<String>> = {
            let open = names(&app);
            app.panes
                .iter()
                .map(|pane| open.get(pane.buffer).cloned())
                .collect()
        };
        assert!(
            watching.contains(&Some("d.rs".to_string())),
            "a pane followed its index instead of its file: {watching:?}"
        );
    }

    #[test]
    fn one_file_reached_two_ways_is_one_buffer() {
        // Identity was `relative_crumbs` — a display path, truncated to four segments. Opening the
        // same file by a second spelling gave it two independent undo histories and two dirty
        // flags over one file on disk, and whichever was saved last won.
        let dir = scratch("identity");
        // `sub` has to exist. Windows collapses `sub/..` lexically before the syscall, so a missing
        // directory works there and nowhere else — this test passed on the machine it was written
        // on and failed on CI, which is the same shape as the `sh` vs `cmd` slip in `picker.rs`.
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut app = editor(&dir, &["a.rs", "b.rs"]);
        let count = app.buffers.len();

        // The same file, named through a directory and back out of it.
        app.activate(Target::File(dir.join("sub").join("..").join("a.rs")));

        assert_eq!(
            app.buffers.len(),
            count,
            "the same file was opened a second time"
        );
        assert_eq!(
            app.state.active_buffer, 0,
            "it switched to the one already open"
        );
    }

    #[test]
    fn a_buffer_opened_under_an_awkward_spelling_is_still_released_by_a_delete() {
        // The ordering trap, and the reason `rebind_buffers` takes indices rather than a path.
        // `vex_project::canonical` hands back its input unchanged once `canonicalize` fails, and it
        // fails the moment the file is gone — on *both* sides of the comparison. Taking the key
        // early is not enough: the buffers' own keys have to be read before the syscall too, so the
        // only thing that can cross it is the answer.
        let dir = scratch("spelling");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut app = editor(&dir, &["b.rs"]);

        // A buffer whose stored path is another spelling of the same file — exactly what session
        // restore produces, since it pushes whatever the session file held without going through
        // the already-open check.
        std::fs::write(dir.join("a.rs"), "contents of a.rs\n").unwrap();
        app.open_in_background(&dir.join("sub").join("..").join("a.rs"), 0, 0);
        assert_eq!(app.buffers.len(), 1);

        let path = dir.join("a.rs");
        let showing = app.buffers_showing(&path);
        std::fs::remove_file(&path).unwrap();
        app.rebind_buffers(&showing, None);

        assert_eq!(
            app.buffer_file(1),
            None,
            "the buffer still names a file that is gone"
        );
    }

    #[test]
    fn a_deleted_file_releases_every_buffer_showing_it_not_just_the_visible_one() {
        // A stored buffer left pointing at a deleted file writes it back into existence the next
        // time that buffer is saved — and the file being deleted from the tree is usually not the
        // one on screen, which is exactly the case this missed.
        let dir = scratch("delete");
        let mut app = editor(&dir, &["a.rs", "b.rs"]);
        assert_eq!(app.state.active_buffer, 1);

        let showing = app.buffers_showing(&dir.join("a.rs"));
        std::fs::remove_file(dir.join("a.rs")).unwrap();
        app.rebind_buffers(&showing, None);

        assert_eq!(
            app.buffer_file(0),
            None,
            "a stored buffer still points at the deleted file"
        );
        assert_eq!(names(&app)[0], "untitled");
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-session-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor with `names` open in order, the last one live.
    fn editor(dir: &std::path::Path, names: &[&str]) -> App {
        for name in names {
            std::fs::write(dir.join(name), format!("contents of {name}\n")).unwrap();
        }
        let (state, _) = new_state(
            Text::from_str(&format!("contents of {}\n", names[0])),
            vec![names[0].to_string()],
            Some(dir.join(names[0]).to_string_lossy().to_string()),
            Mode::Insert,
            None,
            dir.to_path_buf(),
        );
        let mut app = App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &vex::config::Settings::default(),
        );
        for name in &names[1..] {
            app.activate(Target::File(dir.join(name)));
        }
        app
    }

    fn file_names(session: &vex::session::Session) -> Vec<String> {
        session
            .files
            .iter()
            .map(|file| file.path.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn an_untitled_buffer_in_front_does_not_make_the_session_reopen_on_the_wrong_file() {
        // `active` indexes the *written* list, and it was recorded before the buffer was checked
        // for having a path at all. A scratch buffer numbered below the live one therefore left
        // `active` naming whichever file happened to be written next — so a session with an
        // untitled buffer open came back on someone else's file.
        let dir = scratch("untitled");
        let mut app = editor(&dir, &["a.rs", "b.rs", "c.rs"]);

        // The live buffer is untitled, and a file follows it. This is the only shape that breaks:
        // `active` was recorded for the live buffer *before* it turned out to have no path, so the
        // slot it reserved was filled by the next file written.
        app.switch_to(1);
        app.state.file_path = None;
        app.state.path = vec!["untitled".to_string()];

        let session = app.session_snapshot();
        let names = file_names(&session);
        assert_eq!(
            names,
            ["a.rs", "c.rs"],
            "the untitled buffer is not written"
        );
        assert_eq!(
            session.active,
            0,
            "no file was active, so the session must fall back to the first rather than name \
             `{:?}` — which the user was not looking at",
            names.get(session.active)
        );
    }

    #[test]
    fn an_untitled_buffer_ahead_of_the_active_one_does_not_shift_it() {
        // The case that already worked, pinned so the fix above cannot break it: `active` counts
        // *written* files, so a skipped buffer before the live one must not move it.
        let dir = scratch("ahead");
        let mut app = editor(&dir, &["a.rs", "b.rs", "c.rs"]);
        app.buffers[0].file_path = None;
        app.buffers[0].path = vec!["untitled".to_string()];

        let session = app.session_snapshot();
        let names = file_names(&session);
        assert_eq!(names, ["b.rs", "c.rs"]);
        assert_eq!(
            names.get(session.active).map(String::as_str),
            Some("c.rs"),
            "the active file moved because a buffer ahead of it was skipped"
        );
    }

    #[test]
    fn restoring_a_session_keeps_the_order_it_was_written_in() {
        // `open_in_background` appends, so walking the session backwards restored the other files
        // in the opposite order — the tab strip came back mirrored and every buffer number named a
        // different file than it had last time.
        let dir = scratch("order");
        let app = editor(&dir, &["a.rs", "b.rs", "c.rs", "d.rs"]);
        let session = app.session_snapshot();
        assert_eq!(file_names(&session), ["a.rs", "b.rs", "c.rs", "d.rs"]);

        // The real restore path, not a copy of it: a second walk written here would pass whatever
        // `main` did, which is how the defect survived in the first place.
        let mut restored = editor(&dir, &["d.rs"]);
        restored.restore_session(&session);

        let order: Vec<String> = (0..=restored.buffers.len())
            .filter_map(|index| restored.buffer_file(index))
            .map(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(
            order,
            ["d.rs", "a.rs", "b.rs", "c.rs"],
            "the restored buffers are not in the order the session wrote them"
        );
    }

    #[test]
    fn switching_files_closes_the_one_the_server_was_told_about() {
        // Every `didOpen` owes a `didClose`, and none was ever sent: the editor announced each file
        // it visited and never took one back, so a server accumulated every document of a whole
        // session — each frozen at the version last sent, each still answering against a buffer the
        // user may have closed an hour before.
        //
        // Asserted on `opened_document`, which is the editor's record of what the server believes.
        // The notification itself is one line in `close_open_document`; what kept going wrong is
        // knowing *when* to send it.
        let dir = scratch("didclose");
        let mut app = editor(&dir, &["a.rs", "b.rs"]);

        // Pretend the handshake finished and a.rs was announced.
        app.opened_document = Some(("test".to_string(), dir.join("a.rs")));
        app.switch_to(0);

        assert_eq!(
            app.opened_document, None,
            "the server still believes a file is open that the editor has moved off"
        );
    }
}

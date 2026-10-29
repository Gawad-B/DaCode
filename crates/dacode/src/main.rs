//! `dacode` — the editor binary.
//!
//! Wires the layers together: `dacode-text` (storage), `dacode-buffer` (document and undo), `dacode-display`
//! (soft wrap, folding, viewport), `dacode-input` (the grammar), `dacode-syntax`, `dacode-lsp`, `dacode-project`,
//! `dacode-lua` and `dacode-render`. What lives *here* is the event loop, the window, and the wiring: this
//! file owns the things that cannot be tested without a window, and delegates everything that can.

// A release build on Windows is a GUI application, not a console one. Without this, launching from the
// Start Menu opens a black console window behind the editor, which looks like a fault. Debug builds
// keep the console, because that is where `eprintln!` diagnostics go and losing them while developing
// would be the worse trade.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use dacode::chrome::{self, EditorState};
use dacode::config;
use dacode::execute::{Outcome, execute};
use dacode::palette::{Palette, PaletteKind, Target};
use dacode::panes::Panes;
use dacode::prompt::{Command, PromptKind, parse_command};
use dacode::recovery::Recovery;
use dacode_buffer::Document;
use dacode_display::{DisplayMap, ScrollAnchor, WrapMode};
use dacode_input::{Action, Grammar, Key, KeyCode, Registers, Resolution};
use dacode_project::{FileIndex, FileWalk, find_root};
use dacode_render::{Frame, Rect, Renderer};
use dacode_syntax::{Language, SyntaxRequest, SyntaxWorker};
use dacode_text::{CharOffset, Selection, SelectionSet, Text, Version};
use dacode_theme::{Mode, Theme, space};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// Rows of context kept between the cursor and the edge of the viewport.
const SCROLL_MARGIN: usize = 3;

/// How long typing must pause before autosave writes. Read by the write and by the loop's sleep.
const AUTOSAVE_QUIET: std::time::Duration = std::time::Duration::from_millis(900);

/// How long after the last change on the settings page `init.lua` is written.
///
/// Long enough that clicking a font size up five times writes once rather than five times, short
/// enough that quitting straight after a change has nothing left to lose — and quitting writes what
/// is pending anyway. Read by the write and by the loop's sleep, like [`AUTOSAVE_QUIET`].
const SETTINGS_SAVE_AFTER: std::time::Duration = std::time::Duration::from_millis(600);

/// Columns of context kept beside the cursor when the view scrolls sideways.
///
/// Eight, where the vertical margin is three: a row of context is a whole line, a column is one
/// character, and a caret pinned to the exact right edge shows nothing of what it is typing
/// towards. `scroll_to_reveal_column` caps it at half the viewport, so a narrow pane still works.
const H_SCROLL_MARGIN: usize = 8;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // First, so everything after it — the import included — reads the configuration vex left.
    dacode::paths::migrate_from_vex();

    // The downloaded formatters and servers go on the *end* of PATH, so one the user installed is
    // found first — and for everything that looks a program up: the server table, the formatters,
    // the installer's check afterwards, and the terminal's shell. Here, before a single thread
    // exists: `set_var` races every `getenv` on another thread, which once deadlocked the test
    // suite inside glibc. It is the only place the editor changes its own environment.
    if let Some(path) =
        dacode::tools::path_with_tools(std::env::var_os("PATH").as_deref(), &dacode::tools::root())
    {
        // SAFETY: the process is still single-threaded here; nothing has been spawned yet.
        unsafe { std::env::set_var("PATH", path) };
    }

    // `dacode import …` is a separate operation: it rewrites configuration, so it happens when asked
    // rather than at startup.
    if args.first().map(String::as_str) == Some("import") {
        return dacode::import_cli::run(&args);
    }
    // `dacode tools list` and `dacode tools install NAME…`: what the editor's own installer runs.
    if args.first().map(String::as_str) == Some("tools") {
        std::process::exit(dacode::tools::run_cli(&args[1..]));
    }

    // Before anything that could open a window. `dacode --version` used to fall through `positional`
    // as an unrecognised flag and start the editor, which is the one thing a person typing it is
    // certainly not asking for — and a bug report that asks for the version had no way to get one.
    //
    // On a Windows *release* build these print into a console the process does not own: the binary
    // is `windows_subsystem = "windows"`, so a shell that redirects or pipes sees the text and one
    // that does not shows nothing. `dacode import --help` has always behaved this way. Attaching the
    // parent console would fix it and needs FFI that cannot be tested from here, so the limitation
    // is written down rather than half-fixed.
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("dacode {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{}", usage());
        return Ok(());
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

    // `dacode some/dir` opens a *project*, the way `dacode file.rs` opens a file. The directory itself
    // becomes the root rather than searching upwards from it for a `.git`: naming a folder is an
    // explicit choice about what the project is, and second-guessing it would index the parent
    // repository when you asked for one crate of it.
    let folder = argument
        .as_ref()
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_dir())
        .map(|path| dacode_project::canonical(&path));
    let named_something = argument.is_some();
    let path = if folder.is_some() { None } else { argument };

    // With no file named — including when a folder was — the last session for that project is what
    // to open. Naming a file explicitly wins: asking for one file and getting the last three back
    // would be surprising.
    // Started with nothing at all — from a Start Menu entry, a desktop icon, a dock — the project to
    // open is the last one worked in, not whatever directory the launcher happened to point at. On
    // Windows that directory is where the program was installed, which is nobody's project.
    let remembered = (!named_something && !args.iter().any(|arg| arg == "--no-session"))
        .then(|| dacode::session::last_project(&dacode::session::session_dir()))
        .flatten();

    let restore = (path.is_none() && !args.iter().any(|arg| arg == "--no-session"))
        .then(|| {
            let anchor = folder
                .clone()
                .or_else(|| remembered.clone())
                .unwrap_or_else(|| find_root(&std::env::current_dir().unwrap_or_default()));
            dacode::session::load(&dacode::session::session_dir(), &anchor)
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
    eprintln!("dacode: project root {}", root.display());
    dacode::session::remember_project(&dacode::session::session_dir(), &root);

    let event_loop = EventLoop::<dacode::wake::Wake>::with_user_event()
        .build()
        .context("creating the event loop")?;
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

    // Workers call this from their own threads when they have an answer; the loop, asleep with
    // nothing to do, wakes and collects it rather than waiting for the next keypress. Made before
    // the app so that `App::new` can hand it to every worker as it builds them.
    let proxy = event_loop.create_proxy();
    // Coalescing: a language server indexing queues events many times a second, and one wake
    // collects them all.
    let (waker, wake_pending) = dacode::wake::coalescing(move || {
        let _ = proxy.send_event(dacode::wake::Wake);
    });
    let mut app = App::new(
        root.clone(),
        state,
        grammar,
        &starting_settings,
        Some(waker),
    );
    app.wake_pending = wake_pending;
    app.walk = Some(FileWalk::start(root.clone()));
    // Asked at once, so the status line names the branch before the view is ever opened.
    app.refresh_scm(false);
    dacode::recovery::install_panic_hook(app.lifeline.clone());

    // First run: no configuration means nobody has answered the two questions that decide whether
    // this editor is usable for them at all.
    if dacode::welcome::is_first_run(&dacode::config::config_dir())
        && !args.iter().any(|arg| arg == "--no-welcome")
    {
        app.state.welcome = Some(dacode::welcome::Welcome::new());
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
) -> (EditorState, dacode_input::Keymap) {
    // Config decides the theme and metrics unless the caller overrode them (the screenshot path
    // does, to render a specific appearance on demand).
    let settings = config::load();
    if let Some(source) = &settings.theme_source {
        eprintln!("dacode: theme from {source}");
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
            eprintln!("dacode: config: {problem}");
        }
        let count = settings.problems.len();
        let noun = if count == 1 { "problem" } else { "problems" };
        std::iter::once(format!("config: {count} {noun}"))
            .chain(settings.problems.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    });

    let tab_name = path.last().cloned().unwrap_or_else(|| "untitled".into());

    // The file it starts on is opened here rather than through `after_buffer_change`.
    let mut problems = dacode::problems::Store::default();
    problems.set_current(file_path.as_deref().map(std::path::Path::new));
    let state = EditorState {
        mode,
        theme: theme_override.unwrap_or(settings.theme),
        theme_toggled_from: None,
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
        structure: None,
        frame_ms: 0.0,
        worst_ms: 0.0,
        diagnostics: dacode_lsp::Diagnostics::default(),
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
        vertical_goal: None,
        expansions: dacode::chrome::Expansions::default(),
        search: None,
        scm: Default::default(),
        review: None,
        preview: None,
        recording: None,
        diff: dacode::git::Diff::default(),
        blame: dacode::blame::Tracker::new(settings.inline_blame),
        show_diagnostics: false,
        focus: Default::default(),
        terminal: Default::default(),
        problems,
        inlay_hints: Vec::new(),
        show_inlay_hints: settings.inlay_hints,
        welcome: None,
        settings_page: None,
        settings_active: false,
        tree: None,
        autosave: settings.autosave,
        line_numbers: settings.line_numbers,
        pointer: (0.0, 0.0),
        help: None,
        menu: None,
        chrome: dacode::chrome::Chrome {
            transparency: f32::from(settings.transparency) / 100.0,
            zoom: settings.interface_zoom,
            motion: dacode::motion::Motion::new(settings.animations),
            updates: settings.updates,
            ..Default::default()
        },
    };

    (state, settings.keymap)
}

/// Pairing and indentation rules for a file.
///
/// Per-language, because the differences matter: Python indents after `:` and has no braces to
/// balance, while HTML and CSS should not pair quotes the way a programming language does.
fn pair_config(path: Option<&str>, tab_width: usize) -> dacode_buffer::PairConfig {
    use dacode_syntax::Language;

    let mut config = dacode_buffer::PairConfig {
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
///
/// It went on happening after that was fixed, because the list is written here and the flags are
/// read a thousand lines away in [`screenshot`]: `--row`, `--symbols` and `--block` were all added
/// later and none of them was added here, so `--row 12` opened an empty buffer called `12` behind
/// the settings panel it was meant to be scrolling. Nothing failed — the panel still drew — which is
/// why it survived a screenshot of every overlay. `every_value_flag_is_listed` now derives the set
/// from the source rather than trusting anyone to remember.
const VALUE_FLAGS: [&str; 30] = [
    "--which-key",
    "--menu-title",
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
    "--row",
    "--settings-page",
    "--settings-search",
    "--capture-key",
    "--symbols",
    "--block",
    "--transparency",
    "--zoom",
    "--at-ms",
    "--scroll-px",
    "--terminal-tabs",
    "--terminal-height",
    "--terminal-select",
    "--update-offer",
];

/// Every flag `dacode` accepts, grouped, as the one description of its command line.
///
/// `--help` renders this and `docs/reference/cli.md` quotes it, so the two cannot disagree — the
/// same reason [`dacode_input::commands::COMMANDS`] is one table with four readers. Two tests keep it
/// honest in both directions: a flag listed here that the binary does not read is a lie, and a flag
/// the binary reads that is missing from here is undocumented.
///
/// The headless group is long because it is the whole visual test surface, and a person reaching for
/// it is reproducing a screenshot from CI rather than editing a file.
const FLAGS: [(&str, &str, &str); 56] = [
    // Opening.
    (
        "--no-session",
        "",
        "start empty rather than reopening the last session",
    ),
    ("--no-welcome", "", "skip the first-run questions"),
    ("--version", "", "print the version and exit"),
    ("--help", "", "print this and exit"),
    // Headless rendering. `--screenshot` is what turns the rest on.
    ("--screenshot", "PNG", "render one frame to a file and exit"),
    (
        "--size",
        "WxH",
        "the window to render at (default 1600x1000)",
    ),
    ("--light", "", "the light theme"),
    ("--dark", "", "the dark theme"),
    ("--theme", "NAME", "any built-in or installed theme"),
    ("--mode", "NAME", "normal, insert, visual …"),
    ("--no-vim", "", "render with Vim keys off"),
    (
        "--transparency",
        "PERCENT",
        "see-through, over a checkerboard",
    ),
    (
        "--zoom",
        "PERCENT",
        "at an interface zoom, as ctrl-= sets it",
    ),
    ("--empty", "", "no file open"),
    ("--splash", "", "the cold-start screen"),
    ("--at", "LINE", "put the cursor on a line"),
    ("--at-column", "N", "and a column"),
    (
        "--scroll-px",
        "N",
        "scroll N more pixels, part-way into a row",
    ),
    (
        "--at-ms",
        "N",
        "draw the moment N ms into whatever is animating",
    ),
    ("--columns", "N", "wrap width, in columns"),
    ("--no-wrap", "", "soft wrap off"),
    ("--split", "N[h]", "N panes, `h` for stacked"),
    ("--tree", "", "the file tree"),
    ("--settings", "", "the settings tab"),
    (
        "--settings-page",
        "NAME",
        "open the settings tab at a page: appearance, editor, keys …",
    ),
    (
        "--settings-search",
        "QUERY",
        "type a search into the settings tab",
    ),
    ("--row", "N", "select a settings row, scrolling to it"),
    (
        "--capture-key",
        "NAME[=KEYS]",
        "a Keys page row waiting for a key, KEYS pressed into it",
    ),
    (
        "--theme-dropdown",
        "",
        "the settings tab with its theme list open",
    ),
    ("--welcome", "STEP", "the first-run wizard at one step"),
    ("--folded", "", "with a block folded"),
    ("--git", "", "the diff gutter"),
    ("--blame", "", "the blame column and the caret line's blame"),
    ("--palette", "QUERY", "the file palette, filtered"),
    ("--commands", "", "the command palette"),
    ("--search", "QUERY", "the find box"),
    ("--grep", "QUERY", "the search view, with its results"),
    ("--symbols", "QUERY", "the symbol picker"),
    ("--complete", "PREFIX", "the completion popup"),
    ("--diagnostic", "L:C:MSG", "one diagnostic, inline"),
    ("--diagnostics-list", "", "the problems list"),
    (
        "--source-control",
        "",
        "the source control view, over a canned status",
    ),
    (
        "--no-repository",
        "",
        "the source control view in a folder git does not track",
    ),
    ("--diff-view", "", "a diff tab, showing a canned diff"),
    ("--branches", "", "the branch picker, over canned branches"),
    (
        "--terminal",
        "",
        "the terminal panel, showing a fixed transcript",
    ),
    (
        "--terminal-tabs",
        "N",
        "with --terminal: that many terminal tabs",
    ),
    (
        "--terminal-height",
        "PERCENT",
        "with --terminal: the panel dragged to that share",
    ),
    (
        "--terminal-select",
        "R:C-R:C",
        "with --terminal: a mouse selection",
    ),
    ("--help-panel", "", "the F1 key list"),
    (
        "--update-offer",
        "KIND",
        "the bell offering an update: ready, available or package",
    ),
    ("--menu", "", "the menu bar"),
    (
        "--menu-title",
        "TITLE",
        "the menu bar open at a title, or `...`",
    ),
    ("--context-menu", "", "a right-click menu"),
    (
        "--which-key",
        "PREFIX",
        "what can follow a half-typed chord",
    ),
    (
        "--block",
        "LINE:COLUMN",
        "a rectangular selection to that corner",
    ),
];

/// What `--terminal` shows: a prompt, a coloured listing, a build, a diff summary, the sixteen
/// colours as swatches, 256-colour and 24-bit runs, and a prompt with the cursor waiting. Every kind
/// of thing the panel draws, in the order a session would produce them.
const TERMINAL_TRANSCRIPT: &str = concat!(
    "\x1b[1;32myou@dacode\x1b[0m:\x1b[1;34m~/dacode\x1b[0m$ ls\r\n",
    "\x1b[1;34mcrates\x1b[0m  \x1b[1;34mdocs\x1b[0m  \x1b[1;34mweb\x1b[0m  Cargo.lock  Cargo.toml  ",
    "CHANGELOG.md  README.md  \x1b[1;32mbuild.sh\x1b[0m\r\n",
    "\x1b[1;32myou@dacode\x1b[0m:\x1b[1;34m~/dacode\x1b[0m$ cargo test -p dacode-terminal\r\n",
    "\x1b[1;32m   Compiling\x1b[0m dacode-terminal v1.0.0\r\n",
    "\x1b[1;32m    Finished\x1b[0m `test` profile in 4.21s\r\n",
    "test result: \x1b[32mok\x1b[0m. 26 passed; \x1b[1;31m0 failed\x1b[0m; \x1b[2m0 ignored\x1b[0m\r\n",
    "\x1b[1;32myou@dacode\x1b[0m:\x1b[1;34m~/dacode\x1b[0m$ git status --short\r\n",
    " \x1b[31mM\x1b[0m crates/dacode/src/main.rs\r\n",
    "\x1b[32mA\x1b[0m  crates/dacode-terminal/src/lib.rs\r\n",
    "\x1b[4munderlined\x1b[0m \x1b[7m inverse \x1b[0m \x1b[9mstruck\x1b[0m \x1b[3mitalic\x1b[0m ",
    "\x1b[40m  \x1b[41m  \x1b[42m  \x1b[43m  \x1b[44m  \x1b[45m  \x1b[46m  \x1b[47m  ",
    "\x1b[100m  \x1b[101m  \x1b[102m  \x1b[103m  \x1b[104m  \x1b[105m  \x1b[106m  \x1b[107m  \x1b[0m ",
    "\x1b[38;5;208m256-colour\x1b[0m \x1b[38;2;120;200;255mtruecolor\x1b[0m\r\n",
    "\x1b[1;32myou@dacode\x1b[0m:\x1b[1;34m~/dacode\x1b[0m$ ",
);

/// How many of [`FLAGS`] belong to ordinary use; the rest are the headless renderer's.
///
/// The split is checked, not just declared. `--no-vim` was listed here and read only inside
/// [`screenshot`], so `--help` offered it as a way to start the editor without Vim keys and it did
/// nothing at all — the setting comes from the config. A flag in the wrong group is a documented
/// feature that does not exist, which is the failure the whole table was meant to prevent.
const PLAIN_FLAGS: usize = 4;

/// The text `--help` prints.
///
/// Built from [`FLAGS`] rather than written out, because a help text maintained by hand is one that
/// documents last year's flags. Widths are computed from the longest entry so adding a flag cannot
/// leave the columns ragged.
fn usage() -> String {
    let name = |(flag, value, _): &(&str, &str, &str)| {
        if value.is_empty() {
            (*flag).to_string()
        } else {
            format!("{flag} {value}")
        }
    };
    let width = FLAGS
        .iter()
        .map(|entry| name(entry).len())
        .max()
        .unwrap_or(0);
    let render = |group: &[(&str, &str, &str)]| {
        group
            .iter()
            .map(|entry| format!("    {:width$}   {}\n", name(entry), entry.2))
            .collect::<String>()
    };

    format!(
        "dacode {} — a code editor that never stalls on your keystroke\n\
         \n\
         USAGE\n\
         \x20   dacode [FILE]              open a file\n\
         \x20   dacode [FOLDER]            open a folder as the project\n\
         \x20   dacode                     reopen the last project\n\
         \x20   dacode import …            bring settings over from VSCode (`dacode import --help`)\n\
         \x20   dacode tools …             download formatters and servers (`dacode tools list`)\n\
         \n\
         OPTIONS\n\
         {}\n\
         RENDERING WITHOUT A WINDOW\n\
         \x20   Renders one frame to a PNG and exits. This is how the design is checked in CI and\n\
         \x20   over SSH. Everything below needs --screenshot.\n\
         \n\
         {}\n\
         Configuration lives in {}. DACODE_CONFIG_DIR and DACODE_STATE_DIR move it;\n\
         DACODE_NO_SYSTEM_FONTS=1 makes a render reproducible.\n\
         \n\
         Keys: press F1 in the editor, or read CHEATSHEET.md.\n\
         Docs: https://github.com/Gawad-B/DaCode/tree/main/docs\n",
        env!("CARGO_PKG_VERSION"),
        render(&FLAGS[..PLAIN_FLAGS]),
        render(&FLAGS[PLAIN_FLAGS..]),
        if cfg!(windows) {
            "%APPDATA%\\dacode"
        } else {
            "~/.config/dacode"
        },
    )
}

/// How to run a command, as a phrase that fits in the middle of a sentence.
///
/// Not every command has a key, and some deliberately do not — installing a language server is not
/// something to fire off by muscle memory. `key_for` returns `""` for those, and the message that
/// interpolated it read "…DaCode looked for pyright-langserver on PATH.  installs one, by running:",
/// with a hole exactly where the instruction belonged. A sentence naming the long way round is worth
/// far more than one telling you to press nothing.
///
/// Takes `modal` rather than reading it off the editor so this is testable without one, which is the
/// same reason `paths::resolve` takes its platform.
fn how_to_run(command: &str, modal: bool) -> String {
    let Some(found) = dacode_input::commands::by_name(command) else {
        return String::new();
    };
    match found.shown(modal) {
        keys if keys.is_empty() => {
            let palette = dacode_input::commands::by_name("command_palette")
                .map(|command| command.shown(modal))
                .unwrap_or_default();
            format!("the command palette ({palette}), \"{}\",", found.what)
        }
        keys => keys,
    }
}

/// Whether an action changes the text of the document. The settings tab refuses these while it is
/// the tab on screen: the document is behind it, out of sight, and an edit nobody can see is an edit
/// nobody can check.
///
/// With them, the requests that read the hidden caret and answer over the page: completion, hover,
/// definition, references, and a select-all nobody would see.
fn edits_text(action: &Action) -> bool {
    action.is_change()
        || matches!(
            action,
            Action::RequestCompletion
                | Action::Hover
                | Action::GotoDefinition
                | Action::FindReferences
                | Action::SelectAll
                | Action::Undo
                | Action::Redo
                | Action::Rename
                | Action::Format
                | Action::RevertHunk
                | Action::ExpandSnippet
                | Action::ReplayMacro { .. }
                | Action::CodeAction
                | Action::StartReplace
        )
}

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
/// Split on **both** separators. Windows hands the editor `crates\dacode\src\main.rs`, and splitting on
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
const FIXTURE: &str = "crates/dacode-text/src/anchor.rs";

fn load(path: Option<String>) -> Result<(Text, Vec<String>, Option<String>)> {
    // No argument means no buffer: the editor opens empty and says how to open something. Until now
    // it opened a file from this repository, which was a development scaffold from the first
    // milestone and not behavior anyone would want.
    let Some(path) = path else {
        return Ok((Text::from_str(""), vec!["untitled".to_string()], None));
    };

    match dacode::open::read(std::path::Path::new(&path)) {
        dacode::open::Opened::Text(contents) => {
            let crumbs = relative_crumbs(&path);
            Ok((Text::from_str(&contents), crumbs, Some(path)))
        }
        // An empty buffer with the file's name; `App::ensure_preview` gives it its viewer.
        dacode::open::Opened::View(_) => {
            let crumbs = relative_crumbs(&path);
            Ok((Text::from_str(""), crumbs, Some(path)))
        }
        // `dacode newfile.rs` is meant to work, so a path that does not exist yet is a buffer that
        // knows its name. Anything else — too large, binary, another encoding — keeps `file_path`
        // as `None`, because a buffer that could be saved over a file it never managed to read is
        // how an editor destroys something.
        dacode::open::Opened::Refused(why) => {
            let missing = !std::path::Path::new(&path).exists();
            if missing {
                Ok((Text::from_str(""), relative_crumbs(&path), Some(path)))
            } else {
                eprintln!("dacode: {why}");
                Ok((Text::from_str(""), vec!["untitled".to_string()], None))
            }
        }
    }
}

/// How long after the start of a capture its default frame is drawn: past where every spring settles.
const SETTLED: std::time::Duration = std::time::Duration::from_secs(10);

/// The menu `--menu-title` names, for a bar that shows `shown` of its titles.
///
/// `None` is "open nothing": `…` was asked for at a width where every title fits, so there is no
/// overflow to list. It used to open a list holding only the last title, which photographed
/// something the editor never shows at that width. An unknown title is an error rather than File,
/// because a typo that quietly captures a different menu looks exactly like a menu that is broken.
fn menu_for_title(title: &str, shown: usize, modal: bool) -> Result<Option<dacode::menu::Menu>> {
    use dacode::menu::{Menu, TITLES};
    if title == dacode::layout::OVERFLOW || title == "..." {
        return Ok((shown < TITLES.len()).then(|| Menu::from_title(shown, shown, modal)));
    }
    let index = TITLES
        .iter()
        .position(|name| name.eq_ignore_ascii_case(title))
        .with_context(|| {
            format!(
                "--menu-title {title:?} is not a menu; the titles are {} (or … for the overflow)",
                TITLES.join(", ")
            )
        })?;
    Ok(Some(Menu::bar_at(index, modal)))
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
        Some(Theme::linen())
    } else if flag("--dark") {
        Some(Theme::walnut())
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
    // The capture prints the keys the user's config gives commands, as the editor would.
    dacode_input::shortcuts::install(config::load().shortcuts);

    // Land somewhere with content so the cursor line, the coordinate panel and the cursor shape
    // are all exercised rather than sitting at 0:0.
    let cursor = state
        .document
        .text()
        .point_to_char(dacode_text::Point::new(21, 12));
    state.document.set_primary_caret(cursor);
    if mode == Mode::Visual {
        state
            .document
            .set_selections(SelectionSet::single(Selection::new(
                cursor,
                dacode_text::CharOffset(cursor.get() + 24),
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
    // `--zoom PERCENT` draws at an interface zoom, so a capture can show what ctrl-= does.
    if let Some(percent) = value("--zoom").and_then(|v| v.parse::<u16>().ok()) {
        state.chrome.zoom = dacode::chrome::Zoom::new(percent);
    }
    let zoom = state.chrome.zoom.factor();
    // A viewed file, as `App::ensure_preview` would start it — but waited for, since a capture
    // has no event loop to wake: the file is read, then the pages on screen are rendered.
    if let Some(path) = state.file_path.clone() {
        let path = std::path::PathBuf::from(path);
        if let Some(kind) = dacode::viewer::Kind::opened(&path) {
            if !kind.is_drawn() {
                match dacode::viewer::office_text(&path) {
                    Ok(text) => state.document = Document::from_str(&text),
                    Err(why) => state.hover = Some(why),
                }
            }
            let mut preview = dacode::viewer::Preview::open(kind, path, None);
            let window = Rect::new(0.0, 0.0, width as f32, height as f32);
            let slot = dacode::layout::Layout::of(window, zoom, None, 0.0)
                .pane(0, 1, true, zoom)
                .slot;
            let margin = dacode::layout::viewer_margin(zoom);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while kind.is_drawn() && std::time::Instant::now() < deadline {
                preview.poll();
                let placed = preview.placed(slot, margin);
                preview.request(&placed);
                let drawn = placed
                    .iter()
                    .all(|at| preview.pages[at.index].picture.is_some());
                if preview.status != dacode::viewer::Status::Loading && drawn {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            state.preview = Some(preview);
        }
    }
    fit_wrap_columns(&mut state, width as f32, zoom);
    // Again just before drawing, once each view has opened whatever sidebar it opens: the code's
    // width depends on it, and fitting it here alone wrapped a narrow capture past its own edge.
    if let Some(percent) = value("--transparency").and_then(|v| v.parse::<u8>().ok()) {
        state.chrome.transparency = f32::from(percent.min(90)) / 100.0;
    }
    let forced_columns = value("--columns").is_some();
    let refit = |state: &mut EditorState| {
        if !forced_columns {
            fit_wrap_columns(state, width as f32, zoom);
        }
    };

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
        let matches = dacode_buffer::find_all(
            state.document.text(),
            &query,
            &dacode_buffer::SearchOptions::default(),
        );
        let mut prompt = dacode::prompt::Prompt::new(
            dacode::prompt::PromptKind::Search { forward: true },
            cursor,
        );
        prompt.input = query;
        prompt.matches = matches.clone();
        state.search_matches = matches;
        state.prompt = Some(prompt);
    }
    if let Some(query) = value("--grep") {
        let mut panel = dacode::search_panel::SearchPanel::new(&query);
        panel.restart(&state.project_root);
        // A one-shot render can wait for the search; the editor never does.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline && panel.searching() {
            panel.poll();
            std::thread::yield_now();
        }
        panel.poll();
        state.close_sidebar();
        state.chrome.activity = Some(dacode::layout::Activity::Search);
        state.search = Some(panel);
        state.focus = dacode::focus::Focus::Sidebar;
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
                vec![dacode_lsp::lsp_types::Diagnostic {
                    range: dacode_lsp::lsp_types::Range {
                        start: dacode_lsp::lsp_types::Position {
                            line,
                            character: column,
                        },
                        end: dacode_lsp::lsp_types::Position {
                            line,
                            character: end,
                        },
                    },
                    severity: Some(dacode_lsp::lsp_types::DiagnosticSeverity::ERROR),
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
            let target = text.point_to_char(dacode_text::Point::new(away, 0));
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
        let offset = text.point_to_char(dacode_text::Point::new(line, 0));
        state.document.set_primary_caret(offset);
        state.scroll = ScrollAnchor::at_line(line.saturating_sub(2));
    }

    // `--scroll-px N` scrolls on by N pixels the way a touchpad does, through the same function, so a
    // part-scrolled top row — the one place drawing has to clip against the tab bar — can be looked
    // at. Negative goes back up, which draws from the row above.
    if let Some(px) = value("--scroll-px") {
        let px = px
            .parse::<f32>()
            .with_context(|| format!("--scroll-px takes a number of pixels, got {px:?}"))?;
        let row_height = state.typography.row_height();
        state.scroll = dacode::scroll::by_pixels(
            &state.display,
            state.document.text(),
            state.scroll,
            &mut state.chrome.scroll_input,
            px,
            row_height,
        );
    }

    // `--block LINE:COLUMN` draws a rectangular selection from the cursor to there, so the one
    // gesture that has no key can still be captured. Over ragged text on purpose: whether short
    // lines take part is the thing worth looking at.
    if let Some(corner) = value("--block") {
        let (line, column) = corner.split_once(':').unwrap_or((corner.as_str(), "0"));
        let text = state.document.text();
        let head = dacode_text::Point::new(
            line.parse::<usize>().unwrap_or(0).min(text.last_line()),
            column.parse::<usize>().unwrap_or(0),
        );
        let anchor = text.char_to_point(state.cursor());
        state
            .document
            .set_selections(dacode_text::SelectionSet::column(text, anchor, head));
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
        let items: Vec<dacode_lsp::lsp_types::Diagnostic> = sample
            .iter()
            .map(
                |(line, column, message, is_error)| dacode_lsp::lsp_types::Diagnostic {
                    range: dacode_lsp::lsp_types::Range {
                        start: dacode_lsp::lsp_types::Position {
                            line: *line,
                            character: *column,
                        },
                        end: dacode_lsp::lsp_types::Position {
                            line: *line,
                            character: column + 10,
                        },
                    },
                    severity: Some(if *is_error {
                        dacode_lsp::lsp_types::DiagnosticSeverity::ERROR
                    } else {
                        dacode_lsp::lsp_types::DiagnosticSeverity::WARNING
                    }),
                    message: message.to_string(),
                    ..Default::default()
                },
            )
            .collect();
        // The project store as well as the active file's. The panel shows only the file on
        // screen, so that is the one made current.
        if let Some(path) = state.file_path.clone() {
            state
                .problems
                .publish(std::path::Path::new(&path), &items, None);
            state
                .problems
                .set_current(Some(std::path::Path::new(&path)));
        }
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
        let mut session = dacode::completion::Completion::new(
            state.document.text(),
            caret,
            state.document.version(),
        );
        // Shaped like a real rust-analyzer reply — kinds, types, a snippet and a deprecation —
        // because a fixture of bare labels cannot show whether the type column fits, which is the
        // half of the popup this capture exists to check.
        let suggest = |label: &str, kind: &'static str, detail: &str| dacode_lsp::CompletionItem {
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
                dacode_lsp::CompletionItem {
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
        match dacode_syntax::Highlighter::new(language) {
            Ok(mut highlighter) => {
                highlighter.parse(state.document.text());
                let text = state.document.text();
                state.highlights =
                    highlighter.highlights(text, dacode_text::CharOffset::ZERO..text.len_chars());
                state.node_path = highlighter.node_path(text, cursor);
                state.syntax_error = highlighter.has_error();
                state.syntax_version = state.document.version();
                state.structure = highlighter.structure(state.document.version());
            }
            Err(err) => eprintln!("dacode: syntax unavailable: {err}"),
        }
    }

    // `--at-ms N` photographs a transition part-way. The clock is chosen rather than waited for, which
    // is what makes a frame 90 ms into a sidebar sliding open reproducible: every animated value is a
    // pure function of the instant it is read at, so only `state.chrome.now` has to be moved.
    //
    // The requested view's animations are started at `t0` by `start` below, once the view's own
    // state exists and just before the frame is drawn, so that `--at-ms 0` is the first instant of the
    // transition and not its end.
    //
    // Without `--at-ms` the capture is the *settled* view: every existing capture is of a finished
    // screen, and drawn a few milliseconds after `t0` an overlay would be at progress zero, which is
    // invisible. Ten seconds is far past where any spring settles.
    let t0 = std::time::Instant::now();
    let at = match value("--at-ms") {
        Some(ms) => std::time::Duration::from_millis(ms.parse::<u64>().with_context(|| {
            format!("--at-ms takes a whole number of milliseconds, got {ms:?}")
        })?),
        None => SETTLED,
    };
    state.chrome.now = Some(
        t0.checked_add(at)
            .with_context(|| format!("--at-ms {} is too far in the future", at.as_millis()))?,
    );

    let mut headless =
        dacode_render::Headless::new(width, height).context("initializing offscreen rendering")?;
    eprintln!("dacode: font — {}", headless.resolved_face());

    let bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
    // The requested view's transitions start at `t0`, from closed: everything the flags opened is
    // retargeted through the same function the event loop calls, so a capture at `--at-ms 80` is the
    // frame a person would see 80 ms after opening it.
    let start = |state: &mut EditorState| state.sync_motion(bounds, zoom, t0);
    let mut frame = Frame::default();
    if flag("--splash") {
        chrome::draw_splash(&mut frame, &state.theme, &state.typography, bounds, zoom);
    } else if flag("--git") {
        // A canned diff against the file on screen, so the gutter marks are inspectable without
        // depending on what happens to be uncommitted right now.
        state.diff = dacode::git::Diff::from_marks(
            vec![
                (12, dacode::git::Change::Added),
                (13, dacode::git::Change::Added),
                (17, dacode::git::Change::Modified),
                (24, dacode::git::Change::Removed),
                (28, dacode::git::Change::Modified),
            ],
            state.document.version(),
        );
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--blame") {
        // A canned blame, for the same reason as `--git`'s canned diff: what a capture shows must
        // not depend on whoever last committed to the file on screen. Runs of lines from three
        // commits, and a stretch not committed yet, so both kinds of entry are on screen.
        state.blame.column = true;
        state.blame.show(canned_blame(&state));
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--folded") {
        // Fold every top-level block, so the marker and the hidden-line count are inspectable.
        let mut line = 0;
        let text = state.document.text();
        while line <= text.last_line() {
            match dacode_display::fold_range(text, line, state.display.tab_width) {
                Some(range) => {
                    let end = range.end;
                    state.display.folds.close(range);
                    line = end + 1;
                }
                None => line += 1,
            }
        }
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--diagnostics-list") {
        state.show_diagnostics = true;
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--source-control") || flag("--no-repository") {
        // Canned, for the same reason as `--git`: what is uncommitted in this checkout right now
        // is not something a capture may depend on. Read through the real parser, so the capture
        // also shows what it makes of renames, conflicts and a directory with spaces.
        state.open_source_control();
        let version = state.scm.ask();
        let repo = if flag("--no-repository") {
            dacode::scm::Repo::NotARepository
        } else {
            canned_status()
        };
        state.scm.accept(version, repo);
        if flag("--source-control") {
            state.scm.message = "Handle the empty input before parsing".to_string();
            state.scm.caret = state.scm.message.chars().count();
            state.scm.field = dacode::source_control::Field::List;
            state.scm.selected = Some(5);
        }
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--diff-view") {
        state.document = Document::from_str(CANNED_DIFF);
        state.review = Some(dacode::scm::Show::Changes {
            path: "crates/dacode-text/src/anchor.rs".to_string(),
            side: dacode::scm::Side::Unstaged,
        });
        state.path = review_crumbs(state.review.as_ref().expect("just set"));
        state.buffer_tabs = vec![(state.path.last().cloned().unwrap_or_default(), false)];
        state.file_path = None;
        state.highlights.clear();
        state.scroll = ScrollAnchor::at_line(0);
        state.document.set_primary_caret(dacode_text::CharOffset(0));
        let version = state.scm.ask();
        state.scm.accept(version, canned_status());
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--branches") {
        let version = state.scm.ask();
        state.scm.accept(version, canned_status());
        let mut palette = Palette::new(PaletteKind::Branches);
        palette.refresh_branches(&dacode::scm::parse_branches(CANNED_BRANCHES));
        state.palette = Some(palette);
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--terminal") {
        // No shell: a capture must draw the same thing every time, so the transcript is fed to the
        // same parser a live terminal runs, at the size the panel's grid has here.
        state.terminal.open = true;
        state.focus = dacode::focus::Focus::Terminal;
        if let Some(percent) = value("--terminal-height").and_then(|v| v.parse::<f32>().ok()) {
            state.terminal.fraction = (percent / 100.0).clamp(0.0, 1.0);
        }
        let panel = dacode::layout::Layout::of(bounds, zoom, None, state.sidebar_fraction())
            .with_terminal(true, state.terminal.fraction, false, zoom)
            .terminal
            .expect("asked for one");
        let rects = dacode::layout::terminal_panel(panel, &state.typography, zoom);
        let mut emulator = dacode_terminal::Emulator::new(
            dacode_terminal::GridSize::new(rects.columns, rects.rows),
            dacode_terminal::SCROLLBACK,
        );
        emulator.feed(TERMINAL_TRANSCRIPT.as_bytes());
        // `R:C-R:C`, the first cell and the last, both included — as a drag from one to the other.
        if let Some(spec) = value("--terminal-select") {
            let cell = |text: &str| {
                let (row, column) = text.split_once(':')?;
                Some((row.trim().parse().ok()?, column.trim().parse().ok()?))
            };
            if let Some((Some((r0, c0)), Some((r1, c1)))) = spec
                .split_once('-')
                .map(|(from, to)| (cell(from), cell(to)))
            {
                let at = |row, column, right_half| dacode_terminal::CellPoint {
                    row,
                    column,
                    right_half,
                };
                emulator.select(dacode_terminal::SelectKind::Cells, at(r0, c0, false));
                emulator.extend_selection(at(r1, c1, true));
            }
        }
        state.terminal.screen = Some(emulator.screen());
        // The first tab is the transcript's; any more are named as shells would name them.
        let count = value("--terminal-tabs")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 12);
        let others = ["zsh", "cargo watch -x test", "htop", "bash", "pwsh"];
        state.terminal.tabs = std::iter::once("~/dacode".to_string())
            .chain((1..count).map(|n| others[(n - 1) % others.len()].to_string()))
            .collect();
        // With `--settings` too, the page is in front and has the keys, which folds the panel to
        // its header: the case where the terminal used to cover the rows being read.
        if flag("--settings") {
            state.settings_page = Some(dacode::settings_ui::Page::new());
            state.settings_active = true;
            state.focus = dacode::focus::Focus::Editor;
            state.snap_settings_scroll();
        }
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if let Some(step) = value("--welcome") {
        let mut welcome = dacode::welcome::Welcome::new();
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
        // Arriving from the step before, as it would after answering that one.
        state.chrome.motion.snap(
            dacode::motion::Key::Welcome,
            welcome.step.number() as f32 - 1.0,
        );
        state.welcome = Some(welcome);
        start(&mut state);
        let welcome = state.welcome.take().expect("set just above");
        let arrival = chrome::welcome_arrival(&state.chrome.motion, &welcome, state.now());
        chrome::draw_welcome(
            &mut frame,
            &welcome,
            &state.typography,
            bounds,
            zoom,
            arrival,
        );
    } else if let Some(kind) = value("--update-offer") {
        // A canned release, so the offer's wrapping and buttons can be looked at without a newer
        // release existing — and without the capture reaching the network.
        use dacode::update::{Install, Package, Pending, Stage, Staged};
        let release = dacode::update::parse_release(
            r#"{"tag_name": "v1.2.0", "html_url": "https://github.com/Gawad-B/DaCode",
                "assets": [{"name": "dacode-1.2.0-1.x86_64.rpm", "browser_download_url": "x"},
                           {"name": "SHA256SUMS", "browser_download_url": "x"}]}"#,
        )
        .expect("canned release");
        let (install, stage) = match kind.as_str() {
            "package" => (Install::System, Stage::Available),
            "available" => (Install::WindowsInstaller, Stage::Available),
            _ => (
                Install::WindowsInstaller,
                Stage::Ready(Staged {
                    version: "1.2.0".into(),
                    installer: "dacode-setup.exe".into(),
                }),
            ),
        };
        let pending = Pending { release, stage };
        state.chrome.update = Some(pending.offer(&install, Some(Package::Rpm)));
        state.chrome.notify("saved src/main.rs");
        state
            .chrome
            .notify(&pending.offer(&install, Some(Package::Rpm)).message);
        state.chrome.show_notifications = true;
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--help-panel") {
        state.help = Some(dacode::help::Help::new(state.modal));
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--menu") {
        state.menu = Some(dacode::menu::Menu::bar(state.modal));
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if let Some(title) = value("--menu-title") {
        // A named title opens its menu as F10 and the arrows reach it — with the header that names
        // it when it is behind the overflow — and `…` opens the overflow's list, as a click does.
        let bar = dacode::layout::title_bar(
            dacode::layout::title_strip(bounds, zoom),
            zoom,
            &state.typography,
        );
        state.menu = menu_for_title(&title, bar.menus.len(), state.modal)?;
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--context-menu") {
        state.menu = Some(dacode::menu::Menu::context((420.0, 300.0), state.modal));
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if let Some(prefix) = value("--which-key") {
        // The popup is driven by the pending display, exactly as a half-typed chord drives it.
        state.pending = prefix;
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if let Some(query) = value("--symbols") {
        // A canned outline, so the palette's indenting and its kind column are inspectable without
        // a language server — the same reason `--complete` has a canned list.
        let mut palette = dacode::palette::Palette::new(dacode::palette::PaletteKind::Symbols);
        let at = |line: u32| dacode_lsp::lsp_types::Position {
            line,
            character: 11,
        };
        let symbol =
            |name: &str, kind: &'static str, container: &str, depth, line| dacode_lsp::Symbol {
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
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--commands") {
        let mut palette = dacode::palette::Palette::new(dacode::palette::PaletteKind::Commands);
        palette.refresh_commands(state.modal);
        state.palette = Some(palette);
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if flag("--settings")
        // Each of these only means something on the settings tab, so asking for one is asking for
        // the tab. Without this `--settings-page Editor` alone drew the plain editor, which reads
        // as a capture that passed.
        || value("--settings-page").is_some()
        || value("--settings-search").is_some()
        || value("--row").is_some()
        || value("--capture-key").is_some()
        || flag("--theme-dropdown")
    {
        use dacode::settings_ui::{Category, Entry};
        let modal = state.modal;
        let mut page = dacode::settings_ui::Page::new();
        // The real detection, against this machine — the Languages page is a report, and a
        // screenshot of a report with invented contents would verify nothing about whether it can
        // say the true thing.
        let root = find_root(&std::path::PathBuf::from("."));
        let home = dacode::paths::home();
        let loaded = dacode::config::load();
        page.set_reports(
            dacode::settings_ui::detect(&root, Some(&home), &loaded.servers),
            dacode::settings_ui::detect_formatters(&root, Some(&home), &loaded.formatters),
            modal,
        );
        // `--settings-page` opens a page by its name in the tree, since `--row` only moves within one.
        if let Some(name) = value("--settings-page") {
            if let Some(category) = Category::ALL
                .into_iter()
                .find(|category| category.title().eq_ignore_ascii_case(&name))
            {
                page.set_category(category, modal);
            }
        }
        if let Some(query) = value("--settings-search") {
            page.set_query(query, modal);
        }
        // `--row N` selects entry N and scrolls to it, as ↓ would: the only way to see a row below
        // the fold, which on the Keys and Languages pages is most of them.
        let editor = dacode::layout::Layout::of(bounds, zoom, None, 0.0).editor;
        if let Some(row) = value("--row").and_then(|at| at.parse::<usize>().ok()) {
            page.select(row, modal);
            page.reveal(editor, zoom, modal, &state.typography);
        }
        // A command row waiting for its key, as a click leaves it — and with `=KEYS`, those keys
        // pressed into it, as the real key path presses them: a taken key leaves it asking, any
        // other moves the key and leaves the row with its ⟲.
        if let Some(spec) = value("--capture-key") {
            let (name, keys) = spec.split_once('=').unwrap_or((spec.as_str(), ""));
            if let Some(command) = dacode_input::commands::by_name(name) {
                page.set_category(Category::Keys, modal);
                if let Some(at) = page.entries(modal).iter().position(
                    |entry| matches!(entry, Entry::Command(row) if row.name == command.name),
                ) {
                    page.select(at, modal);
                }
                page.reveal(editor, zoom, modal, &state.typography);
                page.start_capture(command);
                for key in Key::parse_sequence(keys) {
                    let step = page.capture_key(key, modal);
                    let mut shortcuts = (*dacode_input::shortcuts::current()).clone();
                    match step {
                        Some(dacode::settings_ui::CaptureStep::Assign(command, keys)) => {
                            shortcuts.assign(command, keys);
                        }
                        Some(dacode::settings_ui::CaptureStep::Unbind(command)) => {
                            shortcuts.unbind(command);
                        }
                        _ => continue,
                    }
                    dacode_input::shortcuts::install(shortcuts);
                }
                page.reveal(editor, zoom, modal, &state.typography);
            }
        }
        // The theme row's list, opened as a click on its control opens it: on the theme in use and
        // scrolled to it, which at a small size is the only way to see that the list scrolls.
        if flag("--theme-dropdown") {
            page.set_category(Category::Appearance, modal);
            if let Some(at) = page
                .entries(modal)
                .iter()
                .position(|entry| *entry == Entry::Field(dacode::settings_ui::Field::Theme))
            {
                page.select(at, modal);
            }
            page.reveal(editor, zoom, modal, &state.typography);
            page.open_dropdown(state.setting_values().theme);
            let values = state.setting_values();
            let rows = page.geometry(editor, zoom, &values, modal, &state.typography);
            page.reveal_dropdown(&rows, editor, zoom, &state.typography);
        }
        state.settings_page = Some(page);
        state.settings_active = true;
        state.snap_settings_scroll();
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
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

        let mut tree = dacode::tree::FileTree::new(root, index.files());
        tree.reveal("crates/dacode-text/src/anchor.rs");
        state.tree = Some(tree);
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    } else if let Some(split) = value("--split") {
        refit(&mut state);
        start(&mut state);
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
                preview: None,
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
            zoom,
        );
    } else {
        refit(&mut state);
        start(&mut state);
        chrome::draw(&mut frame, &state, bounds, zoom);
    }

    headless
        .capture_png(&frame, state.theme.bg, output)
        .with_context(|| format!("writing {output}"))?;
    eprintln!(
        "dacode: wrote {output} — {} quads, {} text runs, {} marks",
        frame.quads.len(),
        frame.texts.len(),
        frame.marks.len()
    );
    Ok(())
}

/// The status `--source-control` draws, as `git status --porcelain=v2 -z --branch` prints it:
/// a conflict, staged changes including a rename, and unstaged ones including untracked files.
fn canned_status() -> dacode::scm::Repo {
    let records = [
        "# branch.oid 4f2a9c1d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39",
        "# branch.head feature/empty-input",
        "# branch.upstream origin/feature/empty-input",
        "# branch.ab +2 -1",
        "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc crates/dacode/src/config.rs",
        "1 M. N... 100644 100644 100644 aaaa bbbb crates/dacode-text/src/anchor.rs",
        "2 R. N... 100644 100644 100644 aaaa aaaa R100 docs/guides/source control.md",
        "docs/guides/git.md",
        "1 A. N... 000000 100644 100644 0000 aaaa crates/dacode/src/scm.rs",
        "1 .M N... 100644 100644 100644 aaaa bbbb crates/dacode-text/src/anchor.rs",
        "1 .M N... 100644 100644 100644 aaaa bbbb README.md",
        "1 .D N... 100644 100644 000000 aaaa 0000 crates/dacode/src/old_panel.rs",
        "? notes/a very long name for a scratch file that will not fit.txt",
        "? crates/dacode/src/source_control.rs",
    ];
    let mut bytes = Vec::new();
    for record in records {
        bytes.extend_from_slice(record.as_bytes());
        bytes.push(0);
    }
    let mut status = dacode::scm::parse_status(&bytes);
    status.root = find_root(&std::path::PathBuf::from("."));
    dacode::scm::Repo::Ready(Box::new(status))
}

/// The diff `--diff-view` shows: two hunks, with lines added, removed and kept.
const CANNED_DIFF: &str =
    "diff --git a/crates/dacode-text/src/anchor.rs b/crates/dacode-text/src/anchor.rs
index 3b18e51..a9c2f04 100644
--- a/crates/dacode-text/src/anchor.rs
+++ b/crates/dacode-text/src/anchor.rs
@@ -12,9 +12,11 @@ use crate::{CharOffset, Text};
 /// A position that survives edits around it.
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
 pub struct Anchor {
-    offset: usize,
+    offset: CharOffset,
+    /// Which side of an insertion at this exact offset the anchor ends up on.
+    bias: Bias,
 }
 
 impl Anchor {
-    pub fn new(offset: usize) -> Self {
-        Self { offset }
+    pub fn new(offset: CharOffset, bias: Bias) -> Self {
+        Self { offset, bias }
     }
@@ -48,7 +50,7 @@ impl Anchor {
     pub fn transform(&mut self, edit: &Edit) {
         if edit.range.end <= self.offset {
-            self.offset = self.offset + edit.text.len() - edit.range.len();
+            self.offset = edit.shift(self.offset);
         }
     }
 }
";

/// The branches `--branches` lists, as `for-each-ref` prints them.
const CANNED_BRANCHES: &str =
    "*\trefs/heads/feature/empty-input\torigin/feature/empty-input\t2 hours ago
 \trefs/heads/main\torigin/main\t3 days ago
 \trefs/heads/terminal-tabs\t\t5 days ago
 \trefs/heads/fix/crlf-revert\t\t3 weeks ago
 \trefs/remotes/origin/HEAD\t\t3 days ago
 \trefs/remotes/origin/main\t\t3 days ago
 \trefs/remotes/origin/feature/empty-input\t\t2 hours ago
 \trefs/remotes/origin/release/0.3\t\t2 months ago
";

/// The blame `--blame` draws: three authors in runs down the file, and lines 12 and 13 not committed.
fn canned_blame(state: &EditorState) -> dacode::blame::Blame {
    let now = dacode::blame::now();
    let day = 86_400;
    let commit = |hash: char, author: &str, age: i64, summary: &str| dacode::blame::Commit {
        hash: std::iter::repeat_n(hash, 40).collect(),
        author: author.into(),
        mail: String::new(),
        time: now - age,
        summary: summary.into(),
    };
    let commits = vec![
        commit(
            'a',
            "Ada Lovelace",
            400 * day,
            "Lay out the first draft of the module",
        ),
        commit(
            'b',
            "Grace Hopper",
            3 * day,
            "Handle the empty input before parsing",
        ),
        commit(
            'c',
            "Linus Torvalds",
            5 * 3_600,
            "Name the error after what the user did",
        ),
        commit(
            '0',
            "Not Committed Yet",
            0,
            "Version of the file from standard input",
        ),
    ];
    let lines = (0..state.document.text().len_lines())
        .map(|line| match line {
            12 | 13 => 3,
            line if line % 17 < 6 => 0,
            line if line % 17 < 11 => 1,
            _ => 2,
        })
        .collect();
    dacode::blame::Blame::new(
        std::path::PathBuf::from(state.file_path.clone().unwrap_or_default()),
        dacode::blame::Parsed { commits, lines },
    )
}

/// Whether an offset falls inside a non-empty selection.
///
/// A free function so it can be tested without a window: the rule it encodes — a right-click inside
/// a selection keeps it — is the difference between a context menu whose `cut` and `copy` mean
/// something and one where they silently take a single character.
fn selection_contains(
    selections: &dacode_text::SelectionSet,
    offset: dacode_text::CharOffset,
) -> bool {
    selections
        .iter()
        .any(|selection| !selection.is_caret() && selection.range().contains(&offset))
}

/// Derives the wrap width from the code area's pixel width.
///
/// Soft wrap is in columns, so the window size has to be translated once per resize rather than
/// assumed. Getting this wrong is how text ends up wrapping in the wrong place after a resize.
fn fit_wrap_columns(state: &mut EditorState, window_width: f32, scale: f32) {
    // From the layout the chrome draws with, not a sidebar width of its own: this kept a private
    // copy of the old right-hand sidebar's size, and copies of layout arithmetic drift — a wrap
    // column that disagrees with the code area is text running under the edge of the window.
    let regions = dacode::layout::Layout::of(
        Rect::new(0.0, 0.0, window_width, 1000.0 * scale),
        scale,
        None,
        // Where the sidebar is going, not where its slide has got to: the text is wrapped once for
        // the width it will have, rather than rewrapped on every frame of the slide.
        if state.sidebar_open() { 1.0 } else { 0.0 },
    )
    // The blame column takes its width from the code, so the wrap has to know about it — or soft
    // wrapped lines run under the right edge by exactly the column's width.
    .with_blame(state.blame_column(0, scale));
    let grid = dacode::layout::CodeGrid::new(
        regions.pane(0, 1, true, scale).code,
        state.typography.code_size * 0.6 * scale,
        state.typography.row_height() * scale,
        0,
        scale,
    );
    state.display.columns = grid.columns().max(20);
}

/// Scrolling and zooming that has not yet added up to a whole step, and the wheel's glide.
#[derive(Debug, Default)]
struct Gestures {
    /// Sideways pixels short of a whole column. Never drawn: the view moves sideways by columns.
    column_input: dacode::scroll::ScrollInput,
    /// Pixels short of a row for a panel that owns the wheel — the palette, the tree, the results.
    /// Apart from the code's, which would otherwise carry a panel's leftovers into the file.
    panel_input: dacode::scroll::ScrollInput,
    /// ctrl-scroll pixels short of a font-size step. A touchpad sends dozens of deltas per gesture,
    /// and a step per delta would take the type from smallest to largest in one flick.
    zoom_input: dacode::scroll::ScrollInput,
    /// Pinch magnification short of a font-size step, which is a tenth.
    pinch: f64,
    /// The wheel's glide on each axis.
    glide: Glide,
    /// The view the remainders were measured against. When the focused view is found to have moved
    /// any other way — a key, a jump, a search, another pane or buffer, a new font size — they are
    /// forgotten and any glide stops: scrolling that follows the cursor is never offset or animated.
    view: Option<View>,
}

/// A wheel notch in flight. Each axis keeps the spring value it last turned into pixels, so each
/// frame scrolls by the spring's movement since the one before; `None` when that axis is still.
#[derive(Debug, Default, Clone, Copy)]
struct Glide {
    /// The pane whose `Scroll` and `ScrollX` keys are gliding.
    pane: usize,
    rows: Option<f32>,
    columns: Option<f32>,
}

/// What a press on the terminal panel started, followed until the button comes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalGesture {
    /// Dragging the panel's top edge.
    Resize,
    /// Selecting text, from a press on the grid.
    Select,
    /// A press the program asked to be told about. Its drags and its release are the program's
    /// too, and a drag is reported only when it reaches another cell — a report per pointer event
    /// would be a write per pixel.
    Report {
        button: dacode_terminal::MouseButton,
        row: usize,
        column: usize,
    },
}

/// What the focused view was when the scroll remainders were last brought up to date.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    pane: usize,
    buffer: usize,
    scroll: ScrollAnchor,
    cursor: CharOffset,
    row_height: f32,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// The window's size while there is no renderer to ask: before the window opens, and in the
    /// tests, which set it to see the chrome at a size other than the default.
    headless_size: (u32, u32),
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
    requested_range: Option<std::ops::Range<dacode_text::CharOffset>>,
    /// The project's files, for the palette.
    index: FileIndex,
    /// The walk that fills the index, while it is running.
    walk: Option<FileWalk>,
    /// The language server for the active buffer, when one could be started.
    /// Every language server running, keyed by program name.
    servers: std::collections::HashMap<String, dacode_lsp::LspClient>,
    /// Which one the buffer on screen belongs to.
    active_server: Option<String>,
    /// Servers named by `dacode.server(...)` in `init.lua`, tried before the ones DaCode knows about.
    configured_servers: Vec<dacode_lsp::Configured>,
    /// Whether each running server answers `textDocument/formatting`, keyed by program.
    ///
    /// Asked at the handshake rather than discovered by trying: pyright says no, and an editor that
    /// asks anyway gets silence and has nothing to tell the user — where knowing means it can run
    /// black instead.
    server_formats: std::collections::HashMap<String, bool>,
    /// Formatters named by `dacode.formatter(...)`, tried before the ones DaCode knows about.
    configured_formatters: Vec<dacode::format::Configured>,
    /// Runs external formatters off the UI thread.
    formatter: dacode::format::FormatWorker,
    /// Runs the command that installs a language server, off the UI thread — `rustup component add`
    /// takes tens of seconds.
    installer: dacode::install::InstallWorker,
    /// The install a second request would carry out, and what it would run. The message on screen
    /// *is* the prompt, as with every other confirmation here.
    confirm_install: Option<dacode::install::Request>,
    /// The version an external format was requested against, so a stale reply is refused.
    formatting_version: Option<dacode_text::Version>,
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
    sent_version: Option<dacode_text::Version>,
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
    edit_version: Option<dacode_text::Version>,
    /// An open folder dialog, if one is up. Polled like every other background service.
    folder_picker: Option<dacode::picker::FolderPicker>,
    /// Snippets loaded from the config directory, for `tab` expansion.
    snippets: Vec<dacode_import::Snippet>,
    /// The tab stops of the snippet being filled in, if one is.
    snippet: Option<dacode::snippet::Session>,
    /// Where the cursor has jumped from, for `ctrl-o` / `ctrl-i`.
    jumps: dacode_input::JumpList,
    /// Recorded action sequences, for `q` and `@`.
    macros: dacode_input::Macros,
    /// The code actions the server last offered, indexed by the palette.
    code_actions: Vec<dacode_lsp::CodeActionItem>,
    /// The symbols behind the outline palette, whichever scope it was opened at.
    symbols: Vec<dacode_lsp::Symbol>,
    /// The version inlay hints were last requested for, so typing does not queue one per keystroke.
    hinted_version: Option<dacode_text::Version>,
    /// Persists dirty buffers so a crash loses seconds, not work.
    recovery: Recovery,
    /// Every dirty buffer's text, where the panic hook can reach it — so a panic loses nothing at
    /// all, rather than the seconds since the last timed write.
    lifeline: dacode::recovery::Lifeline,
    /// What `lifeline` was last filled from, so it is refilled when that changes rather than on
    /// every wake.
    held: Option<Held>,
    /// What each open file looked like on disk when this editor last read or wrote it, keyed by
    /// `buffer_key`. A save compares against it, so another program's change is not overwritten
    /// without anyone being told.
    on_disk: std::collections::HashMap<std::path::PathBuf, dacode::save::Stamp>,
    /// A file whose outside change the user has been warned about, so the next save they ask for
    /// overwrites it rather than warning again.
    overwrite: Option<std::path::PathBuf>,
    /// The message last recorded for the bell, so one that stays on screen is recorded once.
    last_notice: Option<String>,
    /// The language server's last hover answer. It arrives on the same line as the editor's own
    /// messages, but it is documentation rather than news, and the bell is not where to keep it.
    server_hover: Option<String>,
    /// Diffs the buffer against `HEAD`, off the UI thread.
    git: dacode::git::GitWorker,
    /// The version the last diff was asked for, so typing does not spawn a `git show` per keystroke.
    diffed_version: Option<dacode_text::Version>,
    /// Runs `git blame` and `git show` for the active file, off the UI thread.
    blame_worker: dacode::blame::BlameWorker,
    /// Something happened that a fresh blame should follow: a file opened or reloaded, a save, an
    /// outside change. Spent only when a request is actually sent, so a blame turned on later — the
    /// column toggled on with the inline note off — still gets the one it missed.
    ///
    /// Not raised per keystroke. A blame walks history, which on a long-lived file is hundreds of
    /// milliseconds of another process's time, and between these moments the edit log in
    /// [`dacode::blame`] keeps the one held correct.
    blame_wanted: bool,
    /// Whether the soft wrap was last fitted with the blame column taking its width, so the wrap
    /// is refitted when the column comes or goes rather than every idle step.
    blame_fitted: bool,
    /// Runs every source-control command — status, stage, commit, push, a diff for a diff tab —
    /// one at a time and in order, off the UI thread.
    scm: dacode::scm::Worker,
    /// The branches the picker lists, as git last gave them.
    branches: Vec<dacode::scm::Branch>,
    /// Asks GitHub about releases and downloads them, off the UI thread.
    updater: dacode::update::Worker,
    /// How this copy was installed, read once from where the executable is.
    install: dacode::update::Install,
    /// The machine's package format, for a copy a package manager installed.
    package: Option<dacode::update::Package>,
    /// Where the daily stamp, the skipped version and downloads are kept. A field for the same
    /// reason `config_dir` is: a test gives the editor a directory of its own.
    update_dir: std::path::PathBuf,
    /// A newer release the editor knows about, and how far it has got.
    pending_update: Option<dacode::update::Pending>,
    /// "Restart now" was chosen; the idle step, which can end the loop, carries it out.
    restart_requested: bool,
    /// Watches open files for changes made outside the editor.
    watcher: Option<dacode_project::FileWatcher>,
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
    block_origin: Option<dacode_text::Point>,
    /// When and where the last click landed, for recognising a double or triple one.
    ///
    /// The platform does not tell us: `winit` reports presses, and the "is this the second click of
    /// a double?" question is the application's. Both halves are needed — a second click a second
    /// later, or an inch away, is a new click and not a word selection.
    last_click: Option<(std::time::Instant, f32, f32)>,
    /// 1, 2 or 3: caret, word, line. Kept for the whole gesture so a drag after a double-click is
    /// still a word-wise drag.
    click_count: u8,
    /// Whether the last press chose a row of an overlay — the palette, the help panel, a menu.
    ///
    /// That press closes the overlay, so the second press of a double-click (the habit of every
    /// file picker) arrives with nothing in front of the text: it selected a word in the file just
    /// opened, moved the caret off where the file was restored to, and took the keys to the editor.
    /// The rest of the series, and a drag from it, are swallowed; the next single press clears it.
    chosen_by_overlay: bool,
    /// When the document was last changed, so autosave can wait for a pause in typing rather than
    /// writing a half-typed word to disk every few seconds.
    last_edit: Option<std::time::Instant>,
    /// When a setting was last changed and not yet written to `init.lua`. `None` once it is.
    ///
    /// There is no save button and no `ctrl-s` for settings: a change applies at once, and is
    /// written [`SETTINGS_SAVE_AFTER`] after the last one, by the idle step.
    settings_changed_at: Option<std::time::Instant>,
    /// The settings changed on the page since the last write: the only ones a write sets. Every
    /// other value the session holds — a ctrl-+ zoom, a theme toggled by a key, what an import
    /// brought in — is left as the file has it.
    settings_touched: Vec<dacode::settings_ui::Field>,
    /// The settings put back to their default on the page since the last write: their lines are
    /// taken out of the block.
    settings_reset: Vec<dacode::settings_ui::Field>,
    /// What the file said for each setting when it was last loaded or written by the page. A reload
    /// compares against it to tell whether the file itself changed a field the page has pending.
    file_values: dacode::settings_ui::Values,
    /// The text the settings tab last wrote to `init.lua`, so the watcher can tell that write from
    /// someone else's.
    page_wrote: Option<String>,
    /// A settings write is waiting for an open `init.lua` buffer with unsaved edits to be saved or
    /// closed: writing under it would lose one side or the other.
    settings_blocked: bool,
    /// A key was moved on the Keys page since the last write: the next write rewrites the keys block
    /// from the live set ([`dacode_input::shortcuts::current`]), on the same pause as a setting.
    keys_touched: bool,
    /// Where `init.lua` lives. A field rather than a call to `config::config_dir` at each use, so a
    /// test hands the editor a directory of its own instead of writing the user's real config — the
    /// same reason `config::load_from` takes one.
    config_dir: std::path::PathBuf,
    /// Counts event-loop wake-ups, for tracing only.
    wakes: u64,
    /// Whether the last idle step found something moving, so the step after the motion ends can ask for
    /// one more frame: the last one drawn was a hair short of the target, and the loop is about to sleep.
    was_animating: bool,
    /// The instant the last frame was drawn for, so the mouse measures against the geometry that
    /// frame showed while something slides. `None` before the first.
    last_frame: Option<std::time::Instant>,
    /// Whether the window is hidden behind another or minimised, as winit last said. Nothing is
    /// presented then, so asking for frames only makes the loop spin.
    occluded: bool,
    /// The wheel's, the touchpad's and the pinch's state, except the focused pane's vertical
    /// remainder, which drawing reads and so lives in `state.chrome.scroll_input`.
    gestures: Gestures,
    /// Handed to every worker so a finished result wakes the sleeping event loop. `None` in tests,
    /// which have no loop and poll instead.
    waker: Option<dacode::wake::Waker>,
    /// Raised by the coalescing waker and cleared by `user_event` before it collects. Fresh and
    /// unshared when there is no waker, where nothing ever raises it.
    wake_pending: Arc<std::sync::atomic::AtomicBool>,
    /// The terminals' shells, one per tab, in tab order. Here rather than on the state because
    /// they are processes, and the event loop owns those; `state.terminal` is what drawing reads,
    /// and its `active` says which of these is shown.
    terminals: Vec<dacode_terminal::Terminal>,
    /// What a press on the terminal panel started, while the button is held.
    terminal_gesture: Option<TerminalGesture>,
    /// The colours the terminals were last told to answer colour queries with, so a frame only
    /// tells them again when the theme changed.
    terminal_palette: Option<dacode_terminal::Palette>,
    /// Whether the pointer is showing the resize arrows, so the window is told only on a change.
    resize_pointer: bool,
    /// The shells a new terminal tries, when not the platform's own choice. Tests set it to `sh`
    /// (`cmd` on Windows): a test that started `$SHELL` would be testing the developer's zshrc.
    terminal_shells: Option<Vec<dacode_terminal::ShellCommand>>,
    /// Open panes, and which has focus. Always at least one — see `dacode::panes`.
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
    highlights: Vec<dacode_syntax::HighlightSpan>,
    /// What this buffer is when it is a diff or the history rather than a file — see
    /// [`EditorState::review`], which it is swapped with.
    review: Option<dacode::scm::Show>,
    /// What this buffer shows when it is a viewed file — see [`EditorState::preview`].
    preview: Option<dacode::viewer::Preview>,
}

impl ApplicationHandler<dacode::wake::Wake> for App {
    /// A worker finished. Collect what it produced and redraw if that changed anything on screen.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: dacode::wake::Wake) {
        // Cleared first: a result that lands while collecting then raises a new event, instead of
        // being swallowed by a flag that says one is already on its way.
        self.wake_pending
            .store(false, std::sync::atomic::Ordering::Release);
        if self.collect_background() {
            self.request_redraw();
        }
    }

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
            .with_title("DaCode")
            // Asked for only when it is wanted: a transparent window costs the compositor a blend
            // for every pixel, and an X11 session without a compositing manager can show garbage
            // behind one rather than the desktop.
            .with_transparent(self.state.chrome.transparency > 0.0)
            .with_window_icon(dacode::icon::window_icon())
            .with_inner_size(size);

        // The window has to name itself, or the desktop cannot connect it to its installed entry.
        //
        // Wayland has no window icon protocol at all — `with_window_icon` above is ignored there — so
        // the *only* way GNOME finds an icon for a running window is by matching its `app_id` against
        // an installed `.desktop` file. `dacode` matches `dacode.desktop`. On X11 the same string is the
        // WM_CLASS that `StartupWMClass=dacode` in that file refers to. Without this, the running editor
        // shows a generic placeholder in the dock and the alt-tab switcher even though the icon is
        // correctly installed.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            attributes = WindowAttributesExtWayland::with_name(attributes, "dacode", "dacode");
            attributes = WindowAttributesExtX11::with_name(attributes, "dacode", "dacode");
        }

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("dacode: could not create a window: {err}");
                event_loop.exit();
                return;
            }
        };

        // The config is watched like any other file, so editing `init.lua` in DaCode shows the result
        // on save rather than on the next launch.
        let config_file = self.config_dir.join("init.lua");
        if config_file.exists() {
            if let Some(watcher) = &mut self.watcher {
                let _ = watcher.watch(&config_file);
            }
        }

        // The file opened at startup never went through `after_buffer_change`, so this is where it
        // starts being watched. Missing it meant the first file you opened — the one you are most
        // likely to be editing — was the one file changes were not noticed for.
        self.watch_active();

        match Renderer::new(window.clone(), self.state.chrome.transparency > 0.0) {
            Ok(renderer) => {
                // Report a substitution rather than silently rendering a different design.
                eprintln!("dacode: font — {}", renderer.resolved_face());
                // Said, not silently ignored: the setting is on and the window is opaque, and the
                // person who asked for it should know it was the platform that declined.
                if self.state.chrome.transparency > 0.0 && !renderer.is_translucent() {
                    self.state.hover = Some(
                        "transparency is on, but this display cannot show through the window — \
                         it needs a compositor"
                            .to_string(),
                    );
                }
                self.renderer = Some(renderer);
            }
            Err(err) => {
                eprintln!("dacode: could not initialize the GPU: {err:#}");
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
            match SyntaxWorker::spawn_with(language, self.waker.clone()) {
                Ok(worker) => self.syntax = Some(worker),
                // Highlighting is a nicety; the editor must still open the file without it.
                Err(err) => eprintln!("dacode: syntax highlighting unavailable: {err}"),
            }
        }
        self.restore_recovery();
        self.check_for_updates_if_due();
        self.sync_syntax();
        self.start_language_server();
    }

    /// Runs after the event queue drains, before the loop sleeps.
    ///
    /// Background services are polled here — this is the only place that happens without user input.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if std::env::var_os("DACODE_LSP_TRACE").is_some() {
            self.wakes += 1;
            if self.wakes % 20 == 0 {
                eprintln!(
                    "dacode: {} wake-ups, services_busy={}",
                    self.wakes,
                    self.services_busy()
                );
            }
        }
        let mut changed = self.poll_walk();
        changed |= self.collect_background();
        changed |= self.handle_file_changes();
        changed |= self.rewalk_if_project_changed();
        changed |= self.poll_folder_picker();
        changed |= self.poll_autosave();
        if let Some(results) = &mut self.state.results {
            changed |= results.poll();
        }
        changed |= self.poll_search_panel();

        // Motion is the one thing that needs frames with neither input nor a worker to cause them.
        // Redraws are paced to vsync by winit, so the loop turns once per frame only while something
        // moves; the control flow below stays `Wait`, and it is not a timer.
        let now = std::time::Instant::now();
        // Before the bell reads the status line below, so a write that failed is in it this pass.
        changed |= self.save_settings_if_due(now);
        self.settle_scroll_input();
        // Before asking whether anything moves: whatever this batch of events opened or closed has to
        // have started moving for the answer to include it.
        self.sync_motion(now);
        if self.advance_glide(now) {
            changed = true;
            self.sync_syntax();
        }
        changed |= self.wants_frame(now);

        if changed {
            self.request_redraw();
        }
        self.hold_lifeline();
        self.record_notice();

        if std::mem::take(&mut self.restart_requested) && self.restart_for_update() {
            event_loop.exit();
            return;
        }

        event_loop.set_control_flow(self.control_flow(now));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Whatever the last event did to the view is accounted for before this one reads the scroll
        // remainder — a click measured against a remainder a keystroke has made stale lands a row off.
        self.settle_scroll_input();
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
                self.write_before_leaving();
                self.save_session();
                event_loop.exit();
            }

            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),

            // Coming back to the window is when work done elsewhere — a commit in another
            // terminal, a pull in another tool — is most likely to have happened. The watch on the
            // repository usually has it already; this is the cheap check for when it could not.
            WindowEvent::Focused(true) => {
                self.refresh_scm(false);
                self.request_redraw();
            }

            WindowEvent::Occluded(hidden) => {
                self.occluded = hidden;
                if !hidden {
                    self.request_redraw();
                }
            }
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
                let before = self.state.pointer;
                let was_over_terminal_header = self.over_terminal_header();
                self.pointer = (position.x as f32, position.y as f32);
                self.state.pointer = self.pointer;
                self.update_pointer_icon();
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
                    // The bar's own washes — the search box, the bell — follow the pointer too.
                    if self.move_over_menu() || was_over_toolbar || self.over_toolbar() {
                        self.request_redraw();
                    }
                    return;
                }

                // The palette and the help panel light the row under the pointer, and a drag must
                // not reach the text behind them. Redrawn only when that row changes.
                if self.state.palette.is_some() || self.state.help.is_some() {
                    let previous = std::mem::replace(&mut self.state.pointer, before);
                    let was = self.overlay_row_at();
                    self.state.pointer = previous;
                    if was != self.overlay_row_at() || was_over_toolbar || self.over_toolbar() {
                        self.request_redraw();
                    }
                    return;
                }

                // Dragging extends the selection, which is how every editor spells "select this".
                // A gesture the terminal started stays the terminal's wherever the pointer goes,
                // so a selection dragged up over the code does not start selecting the code.
                if self.terminal_gesture.is_some() {
                    if self.dragging
                        || matches!(self.terminal_gesture, Some(TerminalGesture::Report { .. }))
                    {
                        self.drag_terminal();
                        self.request_redraw();
                    }
                } else if self.dragging {
                    self.click(true);
                    self.keep_offset_under_pointer();
                    self.request_redraw();
                } else if was_over_toolbar
                    || self.over_toolbar()
                    || was_over_terminal_header
                    || self.over_terminal_header()
                {
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
                        self.dragging = self.state.menu.is_none()
                            && self.state.welcome.is_none()
                            && self.state.palette.is_none()
                            && self.state.help.is_none()
                            && !self.dropdown_showing();
                        self.click_count = self.count_click();
                        // A gesture whose release never arrived — the button came up outside the
                        // window — must not capture this press's drag.
                        self.terminal_gesture = None;
                        self.click(false);
                        self.keep_offset_under_pointer();
                    }
                    (winit::event::MouseButton::Left, ElementState::Released) => {
                        self.dragging = false;
                        self.block_origin = None;
                        self.release_terminal(dacode_terminal::MouseButton::Left);
                    }
                    // The context menu, which is where a mouse user looks for cut, copy and "go to
                    // definition" before they look for a keyboard shortcut — unless the press is
                    // over a terminal whose program asked for the mouse, which gets it instead.
                    (winit::event::MouseButton::Right, ElementState::Pressed) => {
                        if !self.press_terminal_other(dacode_terminal::MouseButton::Right) {
                            self.open_context_menu()
                        }
                    }
                    (winit::event::MouseButton::Right, ElementState::Released) => {
                        self.release_terminal(dacode_terminal::MouseButton::Right);
                    }
                    (winit::event::MouseButton::Middle, ElementState::Pressed) => {
                        self.press_terminal_other(dacode_terminal::MouseButton::Middle);
                    }
                    (winit::event::MouseButton::Middle, ElementState::Released) => {
                        self.release_terminal(dacode_terminal::MouseButton::Middle);
                    }
                    _ => {}
                }
                self.request_redraw();
            }

            WindowEvent::MouseWheel { delta, .. } => {
                self.mouse_wheel(delta);
                self.request_redraw();
            }

            // Pinch-to-zoom, where the platform reports it: macOS today. winit 0.30 does not deliver
            // the gesture on Wayland or X11, and ctrl-scroll is the way to resize the text there.
            WindowEvent::PinchGesture { delta, phase, .. } => {
                // A new pinch starts from nothing: a leftover from the last one would make the first
                // movement of this one a step, possibly the wrong way.
                if phase == TouchPhase::Started {
                    self.gestures.pinch = 0.0;
                }
                self.pinch(delta);
                self.request_redraw();
            }

            // Dropping a file on the window opens it. Nothing handled this, so dragging a file onto
            // DaCode did nothing at all — which reads as "this is a toy" faster than any missing
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
                if self.key_event(logical_key) {
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
                    self.write_before_leaving();
                    self.save_session();
                    event_loop.exit();
                } else {
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
        settings: &dacode::config::Settings,
        waker: Option<dacode::wake::Waker>,
    ) -> Self {
        // The keys `dacode.keys` gave commands, made the ones this thread runs and prints. Here
        // rather than in `new_state`, which reads the real config even under test.
        dacode_input::shortcuts::install(settings.shortcuts.clone());
        let install = this_install();
        let package = (install == dacode::update::Install::System)
            .then(this_package)
            .flatten();
        Self {
            window: None,
            renderer: None,
            headless_size: (1280, 820),
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
            formatter: dacode::format::FormatWorker::spawn_with(waker.clone()),
            installer: dacode::install::InstallWorker::spawn_with(waker.clone()),
            confirm_install: None,
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
            jumps: dacode_input::JumpList::default(),
            macros: dacode_input::Macros::default(),
            code_actions: Vec::new(),
            symbols: Vec::new(),
            hinted_version: None,
            recovery: Recovery::new(),
            lifeline: dacode::recovery::Lifeline::default(),
            held: None,
            on_disk: buffer_key(state.file_path.as_deref())
                .zip(
                    state
                        .file_path
                        .as_deref()
                        .and_then(|path| dacode::save::Stamp::of(std::path::Path::new(path))),
                )
                .into_iter()
                .collect(),
            overwrite: None,
            last_notice: None,
            server_hover: None,
            git: dacode::git::GitWorker::spawn_with(waker.clone()),
            diffed_version: None,
            blame_worker: dacode::blame::BlameWorker::spawn_with(waker.clone()),
            blame_wanted: true,
            blame_fitted: false,
            scm: dacode::scm::Worker::spawn_with(waker.clone()),
            branches: Vec::new(),
            updater: dacode::update::Worker::spawn_with(
                Box::new(dacode::update::Curl),
                env!("CARGO_PKG_VERSION"),
                waker.clone(),
            ),
            install,
            package,
            update_dir: dacode::paths::state_dir().join("updates"),
            pending_update: None,
            restart_requested: false,
            watcher: match dacode_project::FileWatcher::with_waker(waker.clone()) {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    // Not fatal: the editor works, it just will not notice outside edits.
                    eprintln!("dacode: file watching unavailable: {error}");
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
            chosen_by_overlay: false,
            last_edit: None,
            settings_changed_at: None,
            settings_touched: Vec::new(),
            settings_reset: Vec::new(),
            file_values: state.setting_values(),
            page_wrote: None,
            settings_blocked: false,
            keys_touched: false,
            config_dir: dacode::config::config_dir(),
            wakes: 0,
            was_animating: false,
            last_frame: None,
            occluded: false,
            gestures: Gestures::default(),
            waker,
            wake_pending: Arc::default(),
            terminals: Vec::new(),
            terminal_gesture: None,
            terminal_palette: None,
            resize_pointer: false,
            terminal_shells: None,
            index: FileIndex::new(root),
            // The project walk is started by `run`, not here: a test wants an editor, not an
            // indexing thread over whatever directory it was handed.
            walk: None,
            buffers: Vec::new(),
            state,
        }
    }

    /// Physical pixels per logical one: the desktop's scale factor times the interface zoom. The
    /// only place the zoom is applied, and every other reader of the scale factor goes through it.
    fn scale(&self) -> f32 {
        self.window
            .as_ref()
            .map(|window| window.scale_factor() as f32)
            .unwrap_or(1.0)
            * self.state.chrome.zoom.factor()
    }

    /// Points every animated value at what the state now says, at `now`. See
    /// [`EditorState::sync_motion`]: called after each batch of events and before each frame, which
    /// is often enough that no site that opens or closes something needs to remember to.
    fn sync_motion(&mut self, now: std::time::Instant) {
        let (bounds, scale) = (self.window_bounds(), self.scale());
        self.state.sync_motion(bounds, scale, now);
        self.state.chrome.covered = if self.dropdown_showing() {
            self.settings_dropdown_geometry()
                .map(|geometry| geometry.card)
        } else {
            None
        };
    }

    /// Whether anything is still moving at `now`. The one thing the event loop needs from the motion
    /// engine: while this is true it draws a frame per vsync, and once it is false it sleeps.
    fn animating(&self, now: std::time::Instant) -> bool {
        self.state.chrome.motion.busy(now)
    }

    /// Whether the idle step should ask for a frame for the sake of motion: while something moves, and
    /// once more after it stops, so the editor never goes to sleep showing a value a fraction short of
    /// where it was going. Not asking when nothing moved is what lets the loop sleep.
    ///
    /// Not while the window is occluded or minimised: `redraw` returns early without presenting, so
    /// nothing would block on vsync and the loop would spin until the spring settled. Motion is a
    /// function of the clock, so nothing is lost by not drawing it; the redraw that un-occluding asks
    /// for shows wherever it has got to.
    fn wants_frame(&mut self, now: std::time::Instant) -> bool {
        let moving = self.animating(now);
        let settling = self.was_animating && !moving;
        self.was_animating = moving;
        let drawable = !self.occluded
            && self
                .renderer
                .as_ref()
                .is_none_or(|renderer| renderer.is_visible());
        drawable && (moving || settling)
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

    /// Gives the wheel to whatever is on screen. Returns true when something took it.
    ///
    /// The wheel had no dispatch at all: it scrolled the document whatever was in front of it, so
    /// the settings panel, the key list, the command palette, the file tree, the search results and
    /// the problems list all ignored it — while the *file behind them* scrolled instead. Every other
    /// input already has this ladder; `press` and `click` each ask the same questions in the
    /// same order, and this is the third reader of that order rather than a new opinion about it.
    ///
    /// For every panel but the help one, moving the selection *is* moving the view: their windows
    /// are computed from `selected` inside the same function the drawing reads, so there is no
    /// second copy of the walk here and no new state. Help is the one with a real offset, and it
    /// already had one going unused.
    ///
    /// Zero lines moves nothing and still answers, which is how a touchpad's delta too small to make
    /// a row of the panel's is kept from scrolling the file behind it.
    fn wheel(&mut self, lines: isize) -> bool {
        // Modal surfaces own the wheel wherever the pointer is — the same rule the keyboard uses,
        // and for the same reason: scrolling the file under an open palette is not what the gesture
        // meant.
        if let Some(palette) = &mut self.state.palette {
            palette.move_selection(lines);
            return true;
        }
        if self.state.help.is_some() {
            // Read before the panel is borrowed: the page is the panel's own, from its geometry.
            let rows = self.help_geometry().map_or(1, |geometry| geometry.capacity);
            if let Some(help) = &mut self.state.help {
                help.scroll_by(lines, rows);
            }
            return true;
        }
        // A menu or the wizard swallows it rather than passing it through to the document.
        if self.state.menu.is_some() || self.state.welcome.is_some() {
            return true;
        }
        // The theme dropdown scrolls its own list, and the page behind it stays where it is.
        if self.dropdown_showing() {
            self.scroll_dropdown(lines as f32 * self.row_height_px());
            return true;
        }

        let layout = self.layout();
        let (x, y) = self.pointer;

        // The settings tab scrolls under the pointer as the text it stands in for does: by pixels,
        // clamped by the page, and the spring draws the glide.
        if self.state.settings_showing() && dacode::layout::contains(layout.editor, x, y) {
            let pixels = lines as f32 * self.row_height_px();
            let (scale, modal, typography) =
                (self.scale(), self.state.modal, self.state.typography);
            if let Some(page) = &mut self.state.settings_page {
                page.scroll_by(pixels, layout.editor, scale, modal, &typography);
            }
            return true;
        }

        if let (Some(results), Some(area)) = (&mut self.state.results, layout.results) {
            if dacode::layout::contains(area, x, y) {
                results.move_selection(lines);
                return true;
            }
        }
        // Over the terminal the wheel scrolls its history: up, which is a negative line count
        // here, is back into older output.
        //
        // A program that asked for the mouse is sent the wheel as buttons 64 and 65 instead, at the
        // cell under the pointer — which is how `htop` and `vim` scroll their own lists — unless
        // shift is held, the same way out of its mouse a press has.
        if let Some(panel) = layout
            .terminal
            .filter(|panel| dacode::layout::contains(*panel, x, y))
        {
            let rects = dacode::layout::terminal_panel(panel, &self.state.typography, self.scale());
            let modifiers = self.mouse_modifiers();
            if let Some(terminal) = self.active_terminal() {
                if terminal.mouse_modes().reporting() && !modifiers.shift && lines != 0 {
                    let at = rects.cell_at(x, y);
                    let button = if lines < 0 {
                        dacode_terminal::MouseButton::WheelUp
                    } else {
                        dacode_terminal::MouseButton::WheelDown
                    };
                    for _ in 0..lines.unsigned_abs().min(64) {
                        terminal.mouse(dacode_terminal::MouseEvent {
                            button,
                            action: dacode_terminal::MouseAction::Press,
                            row: at.row,
                            column: at.column,
                            modifiers,
                        });
                    }
                } else {
                    terminal.scroll(-(lines as i32));
                }
            }
            return true;
        }

        // The sidebar holds three different panels; which one is showing is the same question
        // `click_sidebar` asks, in the same order.
        if layout
            .sidebar
            .is_some_and(|sidebar| dacode::layout::contains(sidebar, x, y))
        {
            if let Some(tree) = &mut self.state.tree {
                tree.move_selection(lines);
                return true;
            }
            if self.state.show_diagnostics {
                // The problems list draws from `problems` directly and has no selection to move, so
                // the wheel is swallowed rather than falling through to scroll the file behind it.
                return true;
            }
            if self.state.source_control_showing() {
                let capacity = self.scm_capacity();
                self.state.scm.scroll(lines, capacity);
                return true;
            }
        }

        false
    }

    /// Routes one wheel or touchpad event.
    ///
    /// A touchpad sends pixels, and they are followed one-to-one: whole rows move the scroll anchor
    /// and the rest is drawn as a part-scrolled top row. They used to be divided by forty and
    /// truncated per event, so a touchpad's stream of three-pixel deltas was a stream of zeros and
    /// nothing scrolled at all. A wheel sends notches, which glide: each one moves the target three
    /// rows on, and `advance_glide` turns the spring's travel into pixels a frame at a time.
    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        self.settle_scroll_input();
        let row_height = self.row_height_px();
        let (dx, dy, pixels) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x, y, false),
            MouseScrollDelta::PixelDelta(position) => (position.x as f32, position.y as f32, true),
        };

        // Over a viewed picture the wheel moves the picture, and with ctrl zooms *it* rather than
        // the code font, which the picture is not drawn in. Not under a modal surface, which owns
        // the wheel wherever the pointer is.
        let over_preview = self
            .state
            .preview
            .as_ref()
            .is_some_and(|p| p.kind.is_drawn())
            && !self.state.settings_showing()
            && self.state.palette.is_none()
            && self.state.help.is_none()
            && self.state.menu.is_none()
            && dacode::layout::contains(self.preview_slot(), self.pointer.0, self.pointer.1);
        if over_preview {
            let (down, sideways) = if pixels {
                (-dy, -dx)
            } else {
                (-dy * 3.0 * row_height, -dx * 3.0 * row_height)
            };
            let (down, sideways) = if self.modifiers.shift_key() && sideways == 0.0 {
                (0.0, down)
            } else {
                (down, sideways)
            };
            self.scroll_preview(sideways, down, self.modifiers.control_key());
            return;
        }

        // ctrl-scroll resizes the text, as it does in every browser and every other editor.
        // The wheel handler ignored modifiers entirely, so the one gesture people try first
        // when the type is too small did nothing.
        //
        // Counted in lines and accumulated: a wheel notch is one line and one step, and a touchpad
        // sends dozens of fractions of one per flick. A step per event ran the type from smallest to
        // largest in one gesture.
        if self.modifiers.control_key() {
            let lines = if pixels { dy / row_height } else { dy };
            let steps = self
                .gestures
                .zoom_input
                .add_restarting_on_reversal(lines, 1.0);
            self.zoom(steps);
            return;
        }

        // Sideways. A trackpad and a tilt wheel send an x delta directly; shift plus a plain wheel
        // is the conventional way to send one from a mouse that cannot. Reading only y, and ignoring
        // the modifier, is why a horizontal gesture did nothing. Only where there is somewhere to
        // go sideways: with wrap on, or over a panel, a shifted wheel still scrolls down, as it
        // always did, rather than doing nothing at all.
        let shifted = self.modifiers.shift_key()
            && dx == 0.0
            && self.state.display.mode == WrapMode::None
            && !self.wheel(0);
        let (sideways, down) = if shifted { (-dy, 0.0) } else { (-dx, -dy) };

        // A notch is a whole number of lines, and glides. Anything finer is a touchpad or a
        // free-spinning wheel — X11 and Windows report both as fractions of a line — which already
        // moves smoothly and is followed by the pixel, as a touchpad that reports pixels is.
        // Rounding each fraction to whole lines was the touchpad bug over again: a tenth of a line
        // became nothing, and a fifth became a whole row.
        let notch = !pixels && down.fract() == 0.0 && sideways.fract() == 0.0;
        let (down_px, sideways_px) = if pixels {
            (down, sideways)
        } else {
            (
                down * 3.0 * row_height,
                sideways * 6.0 * self.focused_grid().advance(),
            )
        };

        // Over the settings tab the page takes the pixels as they come, fingers or notch, and the
        // sideways part goes nowhere — there is no text on screen to move sideways.
        // After the modal surfaces, which own the wheel wherever the pointer is — the order `wheel`
        // keeps — so a palette or the help panel over the page scrolls, and the page behind it does not.
        let layout = self.layout();
        let modal_open = self.state.palette.is_some()
            || self.state.help.is_some()
            || self.state.menu.is_some()
            || self.state.welcome.is_some();
        if !modal_open && self.dropdown_showing() {
            self.scroll_dropdown(down_px);
            return;
        }
        if !modal_open
            && self.state.settings_showing()
            && dacode::layout::contains(layout.editor, self.pointer.0, self.pointer.1)
        {
            let (scale, modal, typography) =
                (self.scale(), self.state.modal, self.state.typography);
            if let Some(page) = &mut self.state.settings_page {
                page.scroll_by(down_px, layout.editor, scale, modal, &typography);
            }
            return;
        }

        if down != 0.0 {
            if notch {
                let lines = down as isize * 3;
                if !self.wheel(lines) {
                    self.glide_rows(lines);
                }
            } else {
                // A panel under the pointer takes whole rows of its own; the code takes the pixels.
                let rows = self.gestures.panel_input.add_pixels(down_px, row_height);
                if !self.wheel(rows) {
                    self.gestures.panel_input.clear();
                    // The fingers are on the glass: whatever a wheel notch started is overtaken.
                    self.cancel_glide();
                    self.scroll_pixels(down_px);
                    self.sync_syntax();
                }
            }
        }

        if sideways != 0.0 && self.state.display.mode == WrapMode::None {
            if notch {
                self.glide_columns(sideways as isize * 6);
            } else {
                self.cancel_glide();
                self.scroll_columns_px(sideways_px);
            }
        }
    }

    /// Pinch-to-zoom: a step of the font size per tenth of magnification, the remainder kept.
    fn pinch(&mut self, delta: f64) {
        if !delta.is_finite() {
            return;
        }
        self.gestures.pinch += delta;
        let steps = (self.gestures.pinch / 0.1).trunc();
        self.gestures.pinch -= steps * 0.1;
        self.zoom(steps as isize);
    }

    /// Runs `steps` font-size changes, larger for positive. A handful at most per event: the size is
    /// clamped anyway, and a runaway delta must not turn into a loop.
    fn zoom(&mut self, steps: isize) {
        let action = if steps > 0 {
            Action::IncreaseFontSize
        } else {
            Action::DecreaseFontSize
        };
        for _ in 0..steps.unsigned_abs().min(8) {
            let _ = self.run(action.clone());
        }
    }

    /// The height of a row of code in physical pixels, the unit a touchpad's deltas arrive in.
    fn row_height_px(&self) -> f32 {
        self.state.typography.row_height() * self.scale()
    }

    /// What the focused view is now, for telling whether something other than scrolling moved it.
    fn view(&self) -> View {
        View {
            pane: self.panes.active(),
            buffer: self.state.active_buffer,
            scroll: self.state.scroll,
            cursor: self.state.cursor(),
            row_height: self.row_height_px(),
        }
    }

    /// Moves the focused view by `px` pixels, down for positive, keeping the part of a row left over.
    ///
    /// Through the display map's own walk, which is the only thing that knows what a fold hides.
    fn scroll_pixels(&mut self, px: f32) {
        let row_height = self.row_height_px();
        self.state.scroll = dacode::scroll::by_pixels(
            &self.state.display,
            self.state.document.text(),
            self.state.scroll,
            &mut self.state.chrome.scroll_input,
            px,
            row_height,
        );
        self.gestures.view = Some(self.view());
    }

    /// Moves the view sideways by `px` pixels, in whole columns, keeping the rest.
    fn scroll_columns_px(&mut self, px: f32) {
        let advance = self.focused_grid().advance();
        let columns = self.gestures.column_input.add_pixels(px, advance);
        let before = self.state.scroll.column;
        if columns != 0 {
            self.scroll_sideways(columns);
        }
        // Against an edge the columns were not all taken, and what is left over would only have to be
        // scrolled back through before the view moved the other way.
        let moved = self.state.scroll.column as isize - before as isize;
        let remainder = self.gestures.column_input.remainder();
        if moved != columns || (remainder < 0.0 && self.state.scroll.column == 0) {
            self.gestures.column_input.clear();
        }
        self.gestures.view = Some(self.view());
    }

    /// Sends the focused pane's wheel glide `rows` further.
    ///
    /// A glide that starts from a part-scrolled row makes up the difference in its target, so it ends
    /// on a whole row: after a wheel notch the view is never left a fraction of a line off.
    fn glide_rows(&mut self, rows: isize) {
        let row_height = self.row_height_px();
        let key = self.glide_key(false);
        let target = match self.gestures.glide.rows {
            Some(_) => {
                self.state.chrome.motion.target(key).unwrap_or(0.0) + rows as f32 * row_height
            }
            None => {
                self.state.chrome.motion.snap(key, 0.0);
                self.gestures.glide.rows = Some(0.0);
                rows as f32 * row_height - self.state.chrome.scroll_input.remainder()
            }
        };
        let now = self.state.now();
        self.state
            .chrome
            .motion
            .to(key, target, dacode::motion::Spring::SCROLL, now);
    }

    /// Sends the focused pane's sideways glide `columns` further, landing on a whole column.
    fn glide_columns(&mut self, columns: isize) {
        let advance = self.focused_grid().advance();
        let key = self.glide_key(true);
        let target = match self.gestures.glide.columns {
            Some(_) => {
                self.state.chrome.motion.target(key).unwrap_or(0.0) + columns as f32 * advance
            }
            None => {
                self.state.chrome.motion.snap(key, 0.0);
                self.gestures.glide.columns = Some(0.0);
                columns as f32 * advance - self.gestures.column_input.remainder()
            }
        };
        let now = self.state.now();
        self.state
            .chrome
            .motion
            .to(key, target, dacode::motion::Spring::SCROLL, now);
    }

    /// The motion key the focused pane glides on. A glide left running in another pane is stopped
    /// first: its travel belongs to a view that is no longer the one being scrolled.
    fn glide_key(&mut self, sideways: bool) -> dacode::motion::Key {
        let pane = self.panes.active();
        if self.gestures.glide.pane != pane {
            self.cancel_glide();
            self.gestures.glide.pane = pane;
        }
        if sideways {
            dacode::motion::Key::ScrollX(pane)
        } else {
            dacode::motion::Key::Scroll(pane)
        }
    }

    /// Stops a glide where it has got to. The keys go back to zero, so a value accumulated over a
    /// session never grows large enough for an `f32` to lose the pixel.
    fn cancel_glide(&mut self) {
        let pane = self.gestures.glide.pane;
        let motion = &mut self.state.chrome.motion;
        if self.gestures.glide.rows.take().is_some() {
            motion.snap(dacode::motion::Key::Scroll(pane), 0.0);
        }
        if self.gestures.glide.columns.take().is_some() {
            motion.snap(dacode::motion::Key::ScrollX(pane), 0.0);
        }
    }

    /// Turns the glide's travel since the last call into scrolling. Returns whether the view moved.
    ///
    /// Called once per turn of the loop, which while a glide runs is once per frame. When the spring
    /// arrives the view is settled on the whole row the glide was aimed at.
    fn advance_glide(&mut self, now: std::time::Instant) -> bool {
        let pane = self.gestures.glide.pane;
        let mut moved = false;

        if let Some(last) = self.gestures.glide.rows {
            let key = dacode::motion::Key::Scroll(pane);
            let value = self.state.chrome.motion.get(key, now);
            if value != last {
                self.scroll_pixels(value - last);
                moved = true;
            }
            if self.state.chrome.motion.target(key) == Some(value) {
                let row_height = self.row_height_px();
                self.state.scroll = dacode::scroll::land_on_row(
                    &self.state.display,
                    self.state.document.text(),
                    self.state.scroll,
                    &mut self.state.chrome.scroll_input,
                    row_height,
                );
                self.state.chrome.motion.snap(key, 0.0);
                self.gestures.glide.rows = None;
                self.gestures.view = Some(self.view());
                moved = true;
            } else {
                self.gestures.glide.rows = Some(value);
            }
        }

        if let Some(last) = self.gestures.glide.columns {
            let key = dacode::motion::Key::ScrollX(pane);
            let value = self.state.chrome.motion.get(key, now);
            if value != last {
                self.scroll_columns_px(value - last);
                moved = true;
            }
            if self.state.chrome.motion.target(key) == Some(value) {
                // Land on the nearest column, as the rows do: the sum of a few dozen frame-sized
                // deltas can end a hair short of the last one, and truncation would then drop it.
                let advance = self.focused_grid().advance();
                let remainder = self.gestures.column_input.remainder();
                self.gestures.column_input.clear();
                if remainder.abs() >= advance / 2.0 {
                    self.scroll_sideways(remainder.signum() as isize);
                }
                self.state.chrome.motion.snap(key, 0.0);
                self.gestures.glide.columns = None;
                self.gestures.view = Some(self.view());
            } else {
                self.gestures.glide.columns = Some(value);
            }
        }
        moved
    }

    /// Forgets the scroll remainders and stops any glide if the focused view was moved by anything
    /// but scrolling since they were last brought up to date.
    ///
    /// A remainder is a distance from a particular top row. After the cursor moved, the view jumped
    /// to a search hit, another pane took focus or the font changed size, it is a distance from
    /// something that is no longer there — and scrolling that follows the cursor is never offset or
    /// animated. Asked before every use of the remainders, and once per turn of the loop.
    fn settle_scroll_input(&mut self) {
        let view = self.view();
        if self.gestures.view == Some(view) {
            return;
        }
        self.cancel_glide();
        self.state.chrome.scroll_input.clear();
        self.gestures.column_input.clear();
        self.gestures.view = Some(view);
    }

    /// A click or a drag moves the cursor to the text under the pointer, which must not then slide
    /// away from it by the part-scrolled row. Only the cursor is excused: a click that switched the
    /// buffer or scrolled still settles as anything else would.
    fn keep_offset_under_pointer(&mut self) {
        let view = self.view();
        if let Some(seen) = &mut self.gestures.view {
            let only_the_cursor = View {
                cursor: view.cursor,
                ..*seen
            };
            if only_the_cursor == view {
                seen.cursor = view.cursor;
            }
        }
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

    /// One key press from the window system, and what follows it. Returns true if the editor should
    /// quit.
    fn key_event(&mut self, logical_key: WinitKey) -> bool {
        // Collect the file index *before* handling the key: `ctrl-p` must not open onto an
        // empty list because the walk finished a moment earlier and nobody looked.
        self.poll_walk();

        // A key that is no key of ours — a bare ctrl, shift or alt — does nothing, so nothing must
        // follow it either. It used to reach the cursor reveal below, so holding ctrl to zoom or to
        // ctrl-click a name threw a scrolled view back to the caret, and the click then landed on
        // whatever had moved under the pointer.
        let Some(key) = self.translate(&logical_key) else {
            return false;
        };
        if self.press(key) {
            return true;
        }
        // A key typed into the terminal moved nothing in the text, and revealing the caret would
        // throw a view scrolled for reading back to wherever the caret was left.
        if self.state.palette.is_none()
            && self.state.prompt.is_none()
            && self.state.focus != dacode::focus::Focus::Terminal
        {
            self.reveal_cursor();
            self.refresh_completion();
            self.sync_syntax();
            self.sync_lsp();
        }
        self.persist_recovery(false);
        false
    }

    /// Routes one key, after the window system's event has become a [`Key`].
    ///
    /// Split from `key_event` so a test drives the real routing rather than a copy of it — the
    /// order of the checks in [`App::route_key`] *is* the behaviour, and it is where the file tree
    /// once took the arrow keys from the editor. Focus is settled on both sides: before, so a view
    /// that has closed cannot eat this key, and after, so a key that moved focus (`ctrl-shift-e`,
    /// a search opening its results) closes the completion popup now rather than leaving it drawn
    /// over a caret that no longer has the keys.
    fn press(&mut self, key: Key) -> bool {
        self.state.settle_focus();
        let quit = self.route_key(key);
        self.state.settle_focus();
        quit
    }

    /// The router itself, in the order a key is offered: the menu, then the other overlays (the
    /// palette, a prompt, the help panel), then the wizard, then completion while the text has the
    /// keys, then whichever surface has focus, and only then the grammar. An overlay always comes
    /// before anything that is merely focused, because it is drawn on top of it: a key that reached
    /// a panel under an open card would drive something the user cannot see.
    fn route_key(&mut self, key: Key) -> bool {
        use dacode::focus::Focus;
        // The menu owns the keyboard while it is open — it is something you are choosing from, and
        // a key that fell through would type into the file behind it. First of all: completion,
        // a hover box and a focused results panel each took Enter or the arrows ahead of it, so a
        // menu opened over them from the title bar could be seen and not driven, and Enter
        // accepted a completion into the buffer behind an open Edit menu. `open_menu` closes the
        // palette and refuses over a prompt, so nothing above this can be open with it either.
        // Returns whether to quit: `File ▸ Quit`.
        if self.state.menu.is_some() {
            return self.handle_menu_key(key);
        }
        // While the palette is open it owns the keyboard: typing `i` into a file query must search
        // for `i`, not enter insert mode. The design says mode keys are swallowed, and they are.
        if self.state.palette.is_some() {
            self.handle_palette_key(key);
            return false;
        }
        if self.state.prompt.is_some() {
            return self.handle_prompt_key(key);
        }
        // The help panel owns the keyboard while it is open too, for the same reason. It used to be
        // asked after completion and a focused results panel, so with the panel focused F1 opened
        // the card and ↓ and Enter went on driving the hits hidden underneath it — and since Enter
        // in help runs its row, the stolen Enter opened a file instead.
        if self.state.help.is_some() {
            return self.handle_help_key(key);
        }
        // The wizard owns the whole window while it is up.
        if self.handle_welcome_key(key) {
            return false;
        }
        // A Keys page row waiting for its new key takes every key, ctrl and alt included: the key
        // being given to a command must not also run whatever it runs now.
        if self.capturing_key() {
            self.handle_capture_key(key);
            return false;
        }
        // The theme dropdown is an overlay on the settings tab, so it is asked before anything the
        // page or the text would do with the key.
        if self.dropdown_showing() && self.handle_dropdown_key(key) {
            return false;
        }
        // A focused terminal takes the key before anything that belongs to the text — completion,
        // a hover box, the grammar — because none of those is what the user is talking to. After
        // the overlays: a palette opened from the terminal is typed into, not the shell under it.
        if self.state.focus == Focus::Terminal {
            return self.handle_terminal_key(key);
        }

        // A hover box is dismissed by the next Escape, before it reaches the grammar and collapses
        // selections — one Escape, one effect.
        if self.state.hover.is_some() && key.code == KeyCode::Escape {
            self.state.hover = None;
            return false;
        }
        // Completion is the caret's, so it is asked only while the text has the keys. `press`
        // closes it when focus leaves, but the router does not lean on that: this check is what
        // keeps ↓ in a focused tree moving the tree.
        if self.state.focus == Focus::Editor
            && self.state.completion.is_some()
            && self.handle_completion_key(key)
        {
            return false;
        }

        // Only with focus: the panel used to take Enter and the arrows whenever it was open, so a
        // search left showing turned every newline into "open the selected hit".
        if self.state.focus == Focus::Panel && self.handle_results_key(key) {
            return false;
        }
        // The sidebar's views take keys only while they have focus. They used to be offered every
        // key first just by being open, so the arrows moved through files whatever was clicked.
        if self.state.focus == Focus::Sidebar {
            // Escape hands the keys back to the text before the view sees it, and it is consumed:
            // one Escape, one effect. The tree used to close itself on it, which threw away the
            // place you had reached just to leave it.
            if key.code == KeyCode::Escape && !key.ctrl && !key.alt {
                self.state.focus = Focus::Editor;
                return false;
            }
            if self.handle_scm_key(key) {
                return false;
            }
            if self.handle_search_key(key) {
                return false;
            }
            if self.handle_tree_key(key) {
                return false;
            }
        }
        // The settings tab takes the keys the text would, being where the text would be. Keys with
        // ctrl or alt go on to the grammar, so the palette, `ctrl-w` and the rest still work over it.
        if self.state.focus == Focus::Editor
            && self.state.settings_showing()
            && self.handle_page_key(key)
        {
            return false;
        }
        // A viewed picture likewise: there is no text for the grammar to move through, and the
        // keys it would have taken scroll and zoom the picture instead.
        if !self.state.settings_showing() && self.handle_preview_key(&key) {
            return false;
        }

        // The intercepts below run ahead of the grammar, so they are skipped for a key the user's
        // bindings claim — or while a command is half-typed — or the key given to a command on the
        // Keys page would open a palette instead.
        let intercept = self.grammar.is_idle() && !self.user_claims(key);
        // Opening a palette is a Ctrl binding, so it works from any mode. Shift matters here:
        // `ctrl-shift-p` is the *command* palette and must not be eaten by the file one — which it
        // was, because this intercept ran before the grammar and ignored the modifier.
        if intercept && key.ctrl && !key.shift {
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
        if intercept && key.ctrl && key.code == KeyCode::Char('f') {
            let action = if key.shift {
                Action::SearchProject
            } else {
                Action::StartSearch { forward: true }
            };
            // Moved to another key on the Keys page: the grammar drops it too.
            if !dacode_input::shortcuts::current().suppresses(&action) {
                let rows = self.code_rows();
                execute(&mut self.state, &mut self.registers, &action, rows);
                return false;
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

    /// Keys for the menu bar and the context menu, which own the keyboard while open.
    ///
    /// Returns whether the editor should quit — `file ▸ quit` is a menu entry, and a menu that
    /// swallowed it would be a menu with a broken row in it.
    fn handle_menu_key(&mut self, key: Key) -> bool {
        let modal = self.state.modal;

        match key.code {
            // The key that opened it closes it. Only escape did, which is fine if you know that and
            // a trap if you pressed f10 to find out what it was.
            KeyCode::Escape | KeyCode::Function(10) => self.state.close_menu(),
            KeyCode::Enter => {
                // A row of the overflow's list opens its menu rather than running anything.
                if let Some(menu) = &mut self.state.menu {
                    if menu.follow(menu.selected, modal) {
                        return false;
                    }
                }
                let action = self
                    .state
                    .menu
                    .as_ref()
                    .and_then(|menu| menu.selection().map(|item| item.action.clone()));
                // Closed before running: a command that opens something of its own — the palette, a
                // prompt — must not find the menu still sitting on top of it.
                self.state.close_menu();
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
        // A page is what the panel has room for, which is its own height and not the code area's.
        let rows = self.help_geometry().map_or(1, |geometry| geometry.capacity);
        match key.code {
            KeyCode::Escape | KeyCode::Function(1) => self.state.close_help(),
            // Runs the highlighted key's command, as the palette runs its row: closed first, so a
            // command that opens something of its own does not find the panel on top of it.
            KeyCode::Enter => {
                let chosen = self.state.help.as_ref().and_then(|help| help.selection());
                self.state.close_help();
                if let Some(name) = chosen {
                    self.activate(Target::Command(name));
                }
            }
            code => {
                if let Some(help) = &mut self.state.help {
                    match code {
                        KeyCode::Up => help.move_selection(-1, rows),
                        KeyCode::Down => help.move_selection(1, rows),
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
        // Over the settings tab there is no text to act on: the code menu's rows would run against
        // the document hidden behind it, and the click that places the caret would have changed
        // the setting under the pointer — and written it. A right-click there does nothing.
        if self.state.settings_showing()
            && dacode::layout::contains(layout.editor, self.pointer.0, self.pointer.1)
        {
            return;
        }
        // Over the terminal, the terminal's own menu: the code's rows would act on the document
        // hidden behind the panel. Focus follows, so the paste it offers lands where it says.
        if layout
            .terminal
            .is_some_and(|panel| dacode::layout::contains(panel, self.pointer.0, self.pointer.1))
        {
            self.state.focus = dacode::focus::Focus::Terminal;
            self.state
                .open_menu(dacode::menu::Menu::terminal(self.pointer, self.state.modal));
            return;
        }
        if let Some(sidebar) = layout
            .sidebar
            .filter(|sidebar| dacode::layout::contains(*sidebar, self.pointer.0, self.pointer.1))
            .filter(|_| self.state.tree.is_some())
        {
            if self.state.menu.is_none() {
                // Select the row first, so the menu acts on what was clicked.
                self.click_sidebar(sidebar, self.pointer.0, self.pointer.1, self.scale());
            }
            self.state
                .open_menu(dacode::menu::Menu::tree(self.pointer, self.state.modal));
            return;
        }

        if self.state.menu.is_none() && !self.pointer_is_in_a_selection() {
            self.click(false);
        }
        self.state
            .open_menu(dacode::menu::Menu::context(self.pointer, self.state.modal));
    }

    fn run(&mut self, action: Action) -> bool {
        // A message describes what just happened, so doing something else takes it away. It used to
        // sit until Escape, which meant "opened a folder" was still on screen three files later and
        // every routine action left a box to dismiss by hand.
        //
        // The three confirmations expire *with* it, because the box **is** the prompt — there is no
        // dialog, and "press it again to mean it" has to mean again *now*. Clearing the message
        // while leaving the arm set would be the worst of both: the next ctrl-q would discard
        // unsaved work having shown no warning at all. `confirm_quit` was in fact never cleared by
        // anything, so one warning armed it for the rest of the session.
        //
        // Not for the action that consumes the arm, or the second press could never get through.
        let consuming = matches!(
            action,
            Action::Quit
                | Action::CloseBuffer
                | Action::DeleteFile
                | Action::InstallLanguageServer
                | Action::RestartToUpdate
        );
        // The source-control view's questions expire the same way, except for the press that
        // answers one.
        if !matches!(
            action,
            Action::Git(
                dacode_input::Git::Discard
                    | dacode_input::Git::DiscardAll
                    | dacode_input::Git::Commit
            )
        ) {
            self.state.scm.confirm = None;
        }
        if !consuming {
            self.state.hover = None;
            self.confirm_quit = false;
            self.confirm_close = None;
            self.confirm_delete = None;
            self.confirm_install = None;
        }
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

        // The settings tab stands where the text was, and the text is still there behind it. A
        // `ctrl-z` or `ctrl-v` that reached it would change a file nobody can see, so nothing that
        // edits text runs while the tab is showing — and `ctrl-w` closes the tab, as it would a file.
        if self.state.settings_showing() {
            if action == Action::CloseBuffer {
                self.close_settings_tab();
                return false;
            }
            if edits_text(&action) {
                return false;
            }
        }

        if action.is_change() {
            self.last_change = Some(action.clone());
        }

        let rows = self.code_rows();
        // Recorded before executing, so a macro captures what was asked for rather than what it
        // happened to do — and before the jumplist, so `ctrl-o` inside a macro replays too.
        self.macros.record(&action);

        // A jump remembers where you were *before* it moves you.
        if dacode_input::is_jump(&action) {
            let here = self.here();
            self.jumps.push(here);
        }

        // The unnamed register *is* the system clipboard, so it is filled from outside before a
        // paste and pushed outside after anything that changed it. Done here rather than in
        // `execute`, which must stay free of the machine it runs on to be testable.
        if matches!(action, Action::Paste { register: None, .. }) {
            if let Some(outside) = dacode::clipboard::get() {
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
            dacode::clipboard::set(after);
        }

        // Font size changes the column advance, so the wrap width has to be recomputed or text
        // keeps wrapping at the old width.
        if matches!(
            action,
            Action::IncreaseFontSize
                | Action::DecreaseFontSize
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::ZoomReset
        ) {
            let (width, scale) = (self.window_width(), self.scale());
            fit_wrap_columns(&mut self.state, width, scale);
        }
        // Kept, unlike a ctrl-scroll of the font: someone who needs the interface larger needs it
        // larger tomorrow too, and should not have to find the settings page to say so.
        if matches!(action, Action::ZoomIn | Action::ZoomOut | Action::ZoomReset) {
            self.settings_changed(
                dacode::settings_ui::Field::InterfaceZoom,
                std::time::Instant::now(),
            );
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
    fn visible_range(&self) -> std::ops::Range<dacode_text::CharOffset> {
        let text = self.state.document.text();
        // One row past what fits whole: while part-scrolled that row is drawn too, sliding in at the
        // bottom, and without spans it would sit there in plain ink until it became a whole row. Always
        // the extra row rather than only while part-scrolled, so the range asked for does not change
        // every time the remainder crosses zero.
        let rows = self
            .state
            .display
            .rows_from(text, self.state.scroll, self.code_rows() + 1);

        let start = rows
            .first()
            .map(|row| row.range.start)
            .unwrap_or(dacode_text::CharOffset::ZERO);
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
        let candidates = dacode_lsp::servers_for(&path, &self.configured_servers);
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
        let home = dacode::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        let found = candidates.iter().find_map(|spec| {
            dacode_lsp::resolve_command(&spec.program, &root, home.as_deref()).map(|at| (spec, at))
        });

        let Some((spec, executable)) = found else {
            self.active_server = None;
            // Names the *language*, not a binary nobody has heard of. The old text was
            // `pyright-langserver not installed`, which someone with Python installed reads —
            // reasonably — as "DaCode cannot see my Python".
            self.state.server_status = format!("no {} server", first.language.to_lowercase());

            // The statusline cell is one narrow line, so the part that actually helps — what DaCode
            // looked for and how to install it — goes in the info box, the same place config
            // problems are reported. Once per language per session: useful the first time you open
            // a .py file, nagging by the third.
            let names = candidates
                .iter()
                .map(|spec| spec.program.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if self.missing_servers.insert(first.language.clone()) {
                // A server the user named is a different failure from one DaCode only guessed at:
                // theirs is a path that is wrong, not a program that is missing, and telling them
                // to install pyright when they asked for a venv would be answering the wrong
                // question.
                self.state.hover = Some(if first.configured {
                    format!(
                        "init.lua asks for `{}` for {}, and dacode could not find it. Relative paths \
                         are resolved against the project root ({}). Nothing else served this \
                         file, so dacode looked for {names}.",
                        first.program,
                        first.language,
                        root.display()
                    )
                } else if first.installer().is_some() {
                    // The editor can run this one, so the answer is a command in the editor rather
                    // than a line to go and type somewhere else. There is no marketplace, and
                    // bundling every server would be a gigabyte of tooling for languages nobody
                    // opened — this runs the same line the message used to print, once you agree.
                    format!(
                        "no {} language server. A language server is separate from the {} you have \
                         installed — dacode looked for {names} on PATH. {} installs one, by running: \
                         {}",
                        first.language,
                        first.language,
                        self.how_to_run("install_language_server"),
                        first.install,
                    )
                } else {
                    format!(
                        "no {} language server. A language server is separate from the {} you have \
                         installed — dacode looked for {names} on PATH. Install one with: {}. To name \
                         your own: dacode.server(\"{}\", {{ command = \"…\" }})",
                        first.language, first.language, first.install, first.language_id
                    )
                });
            }
            eprintln!(
                "dacode: no {} language server; looked for {names}",
                first.language
            );
            return;
        };

        let arguments: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        match dacode_lsp::LspClient::start_with(
            &executable,
            &spec.program,
            &arguments,
            &root,
            self.waker.clone(),
        ) {
            Ok(client) => {
                eprintln!(
                    "dacode: starting {} ({}) in {}",
                    spec.program,
                    executable.display(),
                    root.display()
                );
                self.state.server_status = format!("{} starting", spec.program);
                self.servers.insert(spec.program.clone(), client);
                self.active_server = Some(spec.program.clone());
            }
            // Resolution succeeded and the spawn still failed: this is the case where the user is
            // right and DaCode is wrong, and it should read that way rather than blaming their PATH.
            Err(error) => {
                eprintln!("dacode: {error:#}");
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
                !dacode::format::formatters_for(path, &self.configured_formatters).is_empty()
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
        let candidates = dacode::format::formatters_for(&path, &self.configured_formatters);
        let Some(first) = candidates.first().cloned() else {
            self.warn(
                "dacode knows no formatter for this file — name one with dacode.formatter(…)",
            );
            return;
        };

        let root = self.index.root().to_path_buf();
        let home = dacode::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        let found = candidates.iter().find_map(|formatter| {
            dacode::format::resolve(formatter, &root, home.as_deref())
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
                    "init.lua asks for `{}` to format {}, and dacode could not find it.",
                    first.program, first.language
                )
            } else {
                format!(
                    "no {} formatter — dacode looked for {names} on PATH. Install one with: {}",
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
        self.formatter.request(dacode::format::Request {
            formatter,
            executable,
            path: std::path::PathBuf::from(&path),
            root,
            text: self.state.document.text().to_string(),
            version,
        });
    }

    /// Applies a finished external format, if it is still for the text on screen.
    /// Offers to install the language server for this file, and installs it when asked twice.
    ///
    /// Twice, because this runs a package manager. The first request says exactly what will be run —
    /// the same line the "no language server" message already prints — and the second runs it. The
    /// message *is* the prompt, as with closing a dirty buffer and deleting a file; a dialog would
    /// be a bigger change and this cannot do anything by accident.
    fn request_install_server(&mut self) {
        if let Some(request) = self.confirm_install.take() {
            self.state.hover = Some(format!(
                "installing {} — this can take a minute",
                request.program
            ));
            self.installer.request(request);
            return;
        }

        let Some(path) = self.state.file_path.clone() else {
            self.warn("no file open, so there is no language to install a server for");
            return;
        };
        let candidates = dacode_lsp::servers_for(&path, &self.configured_servers);
        let Some(first) = candidates.first() else {
            self.warn("dacode knows no language server for this kind of file");
            return;
        };

        // Already there: the useful answer is why it is not being used, not an offer to install it
        // again.
        if let Some(installed) = candidates
            .iter()
            .find(|spec| dacode_lsp::can_start(&spec.program))
        {
            self.warn(&format!(
                "{} is already installed — {}",
                installed.program, self.state.server_status
            ));
            return;
        }

        let Some(installer) = first.installer().cloned() else {
            // No single command is honest for this one, so the advice is the whole answer. Saying
            // so beats a button that cannot work.
            self.warn(&format!(
                "dacode cannot install a {} server for you — {}",
                first.language, first.install
            ));
            return;
        };

        self.state.hover = Some(format!(
            "install a {} language server? dacode will run: {} — choose it again to go ahead",
            first.language,
            installer.line()
        ));
        self.confirm_install = Some(dacode::install::Request {
            installer,
            program: first.program.clone(),
            language: first.language.clone(),
        });
    }

    /// Arms a download of `tool`, or runs it when it is the one already armed — choosing a row twice,
    /// the same agreement `request_install_server` asks for, because this fetches and runs a program.
    fn offer_download(&mut self, tool: &dacode::tools::Tool, program: String, language: String) {
        let request = dacode::install::Request {
            installer: dacode_lsp::Installer {
                program: dacode_lsp::SELF.to_string(),
                args: vec!["tools".into(), "install".into(), tool.name.into()],
                needs: dacode_lsp::SELF.to_string(),
            },
            program,
            language,
        };
        let armed = self
            .confirm_install
            .as_ref()
            .is_some_and(|armed| armed.installer.args == request.installer.args);
        if armed {
            self.confirm_install = None;
            self.state.hover = Some(format!(
                "downloading {} {} — this can take a minute",
                tool.name, tool.version
            ));
            self.installer.request(request);
            return;
        }
        self.state.hover = Some(format!(
            "download {} {} ({})? dacode will run: {} — choose it again to go ahead",
            tool.name,
            tool.version,
            tool.what,
            request.installer.line()
        ));
        self.confirm_install = Some(request);
    }

    /// Collects an install when it finishes, and starts the server it just put on disk.
    /// Asks about releases at startup, when the setting allows it and a day has passed since the
    /// last time. Never for a cargo build, which is not offered updates and so has no reason to
    /// reach the network. Called from `resumed`, so neither a test nor a `--screenshot` asks.
    fn check_for_updates_if_due(&mut self) {
        use dacode::update;
        if self.state.chrome.updates == update::Policy::Off
            || self.install == update::Install::Cargo
        {
            return;
        }
        let now = update::now_seconds();
        if !update::due(update::last_check(&self.update_dir), now) {
            return;
        }
        // Stamped when asked rather than when answered, so a check that fails offline is not
        // retried on every start of the day.
        update::record_check(&self.update_dir, now);
        self.updater
            .request(update::Request::Check { manual: false });
    }

    /// Collects the updater's answers. Returns whether anything on screen changed.
    fn poll_updates(&mut self) -> bool {
        use dacode::update::{Reply, Stage};
        let mut changed = false;
        while let Some(reply) = self.updater.poll() {
            changed = true;
            match reply {
                Reply::Checked {
                    manual,
                    result: Err(error),
                } => {
                    // A failed check nobody asked for is not news: the network being down is
                    // not something the editor needs to say once a day.
                    if manual {
                        self.state.hover = Some(error);
                    } else {
                        eprintln!("dacode: update check: {error}");
                    }
                }
                Reply::Checked {
                    manual,
                    result: Ok(None),
                } => {
                    if manual {
                        self.state.hover = Some(format!(
                            "DaCode {} is the latest version",
                            env!("CARGO_PKG_VERSION")
                        ));
                    }
                }
                Reply::Checked {
                    manual,
                    result: Ok(Some(release)),
                } => self.offer_release(release, manual),
                Reply::Staged(result) => {
                    let Some(pending) = &mut self.pending_update else {
                        continue;
                    };
                    match result {
                        Ok(staged) if staged.version == pending.release.version => {
                            let message =
                                format!("DaCode {} is ready — restart to update", staged.version);
                            pending.stage = Stage::Ready(staged);
                            self.state.chrome.notify(&message);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            pending.stage = Stage::Manual(error.clone());
                            self.state.chrome.notify(&error);
                        }
                    }
                }
            }
        }
        if changed {
            self.refresh_offer();
        }
        changed
    }

    /// A newer release has been found: offered, downloaded or said, by how this copy was installed.
    fn offer_release(&mut self, release: dacode::update::Release, manual: bool) {
        use dacode::update::{self, Install, Stage};
        let version = release.version.clone();
        // A skipped version stays skipped until something newer comes — unless asked for by
        // name, through the command.
        if !manual && update::skipped(&self.update_dir).as_deref() == Some(version.as_str()) {
            return;
        }
        if self.install == Install::Cargo {
            if manual {
                self.state.hover = Some(format!(
                    "DaCode {version} is out — this copy was built with cargo, so update it from                      its source"
                ));
            }
            return;
        }
        // Already on its way or ready: a second check must not start it again.
        if let Some(pending) = &self.pending_update {
            if pending.release.version == version
                && matches!(pending.stage, Stage::Downloading | Stage::Ready(_))
            {
                if manual {
                    self.state.chrome.show_notifications = true;
                }
                return;
            }
        }
        let Some(pending) = update::found(
            release,
            &self.install,
            self.package,
            self.state.chrome.updates,
        ) else {
            return;
        };
        if pending.stage == Stage::Downloading {
            self.start_download(&pending.release);
        }
        let message = pending.offer(&self.install, self.package).message;
        self.pending_update = Some(pending);
        self.state.chrome.notify(&message);
        if manual {
            self.state.chrome.show_notifications = true;
        }
    }

    fn start_download(&mut self, release: &dacode::update::Release) {
        use dacode::update;
        if let Some(asset) = update::pick_asset(release, &self.install, self.package) {
            self.updater.request(update::Request::Stage {
                release: release.clone(),
                asset: asset.clone(),
                dir: self.update_dir.clone(),
            });
        }
    }

    /// What the bell's panel offers, from the pending update.
    fn refresh_offer(&mut self) {
        self.state.chrome.update = self
            .pending_update
            .as_ref()
            .map(|pending| pending.offer(&self.install, self.package));
    }

    /// The update commands and the offer's buttons.
    fn update_action(&mut self, action: &Action) {
        use dacode::update::{self, Install, Stage};
        match action {
            Action::CheckForUpdates => {
                update::record_check(&self.update_dir, update::now_seconds());
                self.updater
                    .request(update::Request::Check { manual: true });
                self.state.hover = Some("checking for updates…".to_string());
            }
            Action::DownloadUpdate => {
                let Some(pending) = self.pending_update.clone() else {
                    self.state.hover = Some("no update is on offer".to_string());
                    return;
                };
                if self.install == Install::System {
                    // The package is root's to install. The browser downloads it to where the
                    // printed command expects to find it.
                    let url = update::pick_asset(&pending.release, &self.install, self.package)
                        .map_or(pending.release.page.clone(), |asset| asset.url.clone());
                    self.open_in_browser(&url);
                } else if pending.stage == Stage::Available && self.install.updates_itself() {
                    self.start_download(&pending.release);
                    if let Some(pending) = &mut self.pending_update {
                        pending.stage = Stage::Downloading;
                    }
                    self.refresh_offer();
                }
            }
            Action::UpdateReleaseNotes => {
                let url = self.pending_update.as_ref().map_or_else(
                    || "https://github.com/Gawad-B/DaCode/releases".to_string(),
                    |pending| pending.release.page.clone(),
                );
                self.open_in_browser(&url);
            }
            Action::SkipUpdate => {
                let Some(pending) = self.pending_update.take() else {
                    return;
                };
                let version = pending.release.version;
                self.state.hover = Some(match update::skip(&self.update_dir, &version) {
                    Ok(()) => {
                        format!("DaCode {version} skipped — a later version will still be offered")
                    }
                    Err(error) => format!("could not remember skipping {version}: {error}"),
                });
                self.refresh_offer();
            }
            Action::RestartToUpdate => {
                if matches!(
                    self.pending_update.as_ref().map(|pending| &pending.stage),
                    Some(Stage::Ready(_))
                ) {
                    self.restart_requested = true;
                } else {
                    self.state.hover = Some("no update is ready to install yet".to_string());
                }
            }
            _ => {}
        }
    }

    /// Opens a page in the user's browser. The child is waited for on a thread of its own, so it
    /// does not linger as a zombie and the UI thread never waits on it.
    fn open_in_browser(&mut self, url: &str) {
        let (program, args) = dacode::update::browser(url, cfg!(windows));
        match dacode::process::command(program).args(&args).spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => {
                self.state.hover = Some(format!("could not open {url}: {error}"));
            }
        }
    }

    /// "Restart now": the quit path, then the new version. Returns whether the editor should exit.
    ///
    /// Refuses once with unsaved work, exactly as quitting does, so the second press means it.
    /// Then the session and every dirty buffer's recovery copy are written — the same as any quit,
    /// which the new version restores — and only then is the update installed. If that fails, the
    /// editor stays open as it was and says why.
    fn restart_for_update(&mut self) -> bool {
        use dacode::update::{self, Stage};
        let Some(Stage::Ready(staged)) = self
            .pending_update
            .as_ref()
            .map(|pending| pending.stage.clone())
        else {
            return false;
        };
        let unsaved = self.unsaved_count();
        if unsaved > 0 && !self.confirm_quit {
            self.confirm_quit = true;
            self.state.hover = Some(format!(
                "{unsaved} unsaved file{} — {} saves this one, or Restart now again to leave them \
                 in the recovery file",
                if unsaved == 1 { "" } else { "s" },
                self.key_for("save")
            ));
            self.request_redraw();
            return false;
        }
        self.write_before_leaving();
        self.save_session();
        let relaunch = update::Relaunch {
            args: std::env::args_os().skip(1).collect(),
            dir: std::env::current_dir().unwrap_or_else(|_| self.index.root().to_path_buf()),
        };
        match update::apply(&staged, &self.install, Some(&relaunch)) {
            Ok(()) => true,
            Err(error) => {
                self.state.chrome.notify(&error);
                self.state.hover = Some(error);
                self.request_redraw();
                false
            }
        }
    }

    fn poll_installer(&mut self) -> bool {
        let Some(installed) = self.installer.poll() else {
            return false;
        };
        self.state.hover = Some(installed.message);
        // The Languages page reports what is on disk; it was asked when the page opened.
        if self.state.settings_page.is_some() {
            let servers = self.detect_servers();
            let formatters = self.detect_formatters();
            let modal = self.state.modal;
            if let Some(page) = &mut self.state.settings_page {
                page.set_reports(servers, formatters, modal);
            }
        }

        if installed.ok {
            // The point of installing it. Without this the editor would sit there knowing a server
            // exists and not using it until the file was reopened.
            self.start_language_server();
            self.sync_lsp();
        }
        true
    }

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
        match dacode::format::narrow(&before, &text) {
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
                let edit = dacode_text::Edit::replace(
                    dacode_text::CharOffset(range.start)..dacode_text::CharOffset(range.end),
                    replacement,
                );
                let applied = self.state.mutate(|document| {
                    document.apply(
                        dacode_text::Transaction::new(vec![edit]),
                        dacode_buffer::EditKind::Discrete,
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
            self.write_active(Saving::AfterFormat);
        }
        self.reveal_cursor();
        true
    }

    /// What the settings tab's Languages page reports about language servers.
    ///
    /// Gathered when the tab opens rather than while drawing: it walks `PATH` once per program,
    /// which is bounded but not free.
    fn detect_servers(&self) -> Vec<dacode::settings_ui::ServerRow> {
        let home = dacode::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        dacode::settings_ui::detect(self.index.root(), home.as_deref(), &self.configured_servers)
    }

    /// The same, for external formatters.
    fn detect_formatters(&self) -> Vec<dacode::settings_ui::ServerRow> {
        let home = dacode::paths::home();
        let home = (!home.as_os_str().is_empty()).then_some(home);
        dacode::settings_ui::detect_formatters(
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
    fn active_client(&self) -> Option<&dacode_lsp::LspClient> {
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
        if client.state() == dacode_lsp::State::Ready {
            if let Err(error) = client.did_close(&path) {
                eprintln!("dacode: didClose failed: {error:#}");
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
        if client.state() == dacode_lsp::State::Ready {
            if let Some(path) = self.state.file_path.clone() {
                let path = std::path::PathBuf::from(path);
                let text = self.state.document.text().to_string();

                match self.sent_version {
                    None => {
                        let language = dacode_lsp::server_for(&path.to_string_lossy(), configured)
                            .map(|spec| spec.language_id)
                            .unwrap_or_else(|| "plaintext".to_string());
                        match client.did_open(&path, &language, buffer_version.0 as i32, &text) {
                            // Recorded only on success, so a failed open cannot leave a close owing
                            // for a document the server never heard about.
                            Ok(()) => self.opened_document = Some((name.clone(), path.clone())),
                            Err(error) => eprintln!("dacode: didOpen failed: {error:#}"),
                        }
                        self.sent_version = Some(buffer_version);
                    }
                    Some(sent) if sent != buffer_version => {
                        if let Err(error) = client.did_change(&path, buffer_version.0 as i32, &text)
                        {
                            eprintln!("dacode: didChange failed: {error:#}");
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
        if client.state() == dacode_lsp::State::Ready
            && self.state.show_inlay_hints
            && self.hinted_version != Some(buffer_version)
        {
            if let Some(path) = self.state.file_path.clone() {
                let text = self.state.document.text();
                let last = text.last_line();
                let range = dacode_lsp::lsp_types::Range {
                    start: dacode_lsp::lsp_types::Position {
                        line: 0,
                        character: 0,
                    },
                    end: dacode_lsp::lsp_types::Position {
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
            if client.state() == dacode_lsp::State::Ready {
                if let Some(path) = self.state.file_path.clone() {
                    let path = std::path::PathBuf::from(path);
                    let position =
                        dacode_lsp::to_lsp(self.state.document.text(), self.state.cursor());
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
                            let range = dacode_lsp::lsp_types::Range {
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
                        eprintln!("dacode: request failed: {error:#}");
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
        if client.state() == dacode_lsp::State::Ready {
            if let Some((at, label, raw)) = self
                .state
                .completion
                .as_mut()
                .and_then(dacode::completion::Completion::wants_resolving)
            {
                if let Err(error) = client.resolve_completion(at, &label, &raw) {
                    eprintln!("dacode: resolve failed: {error:#}");
                }
            }
        }

        let active_path = self
            .state
            .file_path
            .as_ref()
            .map(|path| dacode_project::canonical(std::path::Path::new(path)));

        let polled = client.poll();
        if !polled.is_empty() && std::env::var_os("DACODE_LSP_TRACE").is_some() {
            eprintln!("dacode: polled {} lsp events", polled.len());
        }
        for event in polled {
            changed = true;
            match event {
                dacode_lsp::ServerEvent::Ready {
                    server_name,
                    formats,
                } => {
                    eprintln!("dacode: {server_name} ready (formatting: {formats})");
                    self.state.server_status = server_name;
                    // Keyed by the *program*, not by the name the server calls itself: those differ
                    // — pyright-langserver reports "Pyright" — and `active_server` is the program.
                    self.server_formats.insert(name.clone(), formats);
                    // The handshake finished after the last send, so open the document now.
                    self.sent_version = None;
                }
                dacode_lsp::ServerEvent::Diagnostics {
                    path,
                    version,
                    diagnostics,
                } => {
                    // Only for the file on screen: a server reports for the whole project while it
                    // indexes.
                    let matches_active = active_path.as_deref() == Some(path.as_path());
                    if !matches_active && std::env::var_os("DACODE_LSP_TRACE").is_some() {
                        eprintln!(
                            "dacode: diagnostics ignored — {} is not the active {:?}",
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
                                "dacode: diagnostics — {errors} errors, {warnings} warnings (v{})",
                                version.unwrap_or(buffer_version.0 as i32)
                            );
                            self.state.diagnostics_version = dacode_text::Version(
                                version.unwrap_or(buffer_version.0 as i32) as u64,
                            );
                        } else if std::env::var_os("DACODE_LSP_TRACE").is_some() {
                            eprintln!("dacode: diagnostics refused as stale");
                        }
                    }
                }
                dacode_lsp::ServerEvent::Completion { items, incomplete } => {
                    let caret = self.state.cursor();
                    let text = self.state.document.text().clone();
                    if let Some(completion) = &mut self.state.completion {
                        completion.offer(items, incomplete, &text, caret);
                    }
                }
                dacode_lsp::ServerEvent::CompletionResolved { at, label, item } => {
                    if let Some(completion) = &mut self.state.completion {
                        completion.resolved(at, &label, item);
                    }
                }
                dacode_lsp::ServerEvent::Symbols { query, symbols } => {
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
                dacode_lsp::ServerEvent::Hover { text } => {
                    let shown = if text.trim().is_empty() {
                        "no information here".to_string()
                    } else {
                        text
                    };
                    self.server_hover = Some(shown.clone());
                    self.state.hover = Some(shown);
                }
                dacode_lsp::ServerEvent::Definition { path, position } => {
                    self.jump_to(path, position);
                }
                dacode_lsp::ServerEvent::Rename { new_name, edit } => {
                    self.apply_server_edit(
                        &format!("renamed to `{new_name}`"),
                        |state, version| {
                            let path = state.file_path.clone().map(std::path::PathBuf::from);
                            state.mutate(|document| {
                                dacode::edits::apply_workspace_edit(
                                    document,
                                    path.as_deref(),
                                    &edit,
                                    version,
                                )
                            })
                        },
                    );
                }
                dacode_lsp::ServerEvent::Formatting { edits } => {
                    self.apply_server_edit("formatted", |state, version| {
                        state
                            .mutate(|document| {
                                dacode::edits::apply_to_document(document, &edits, version)
                            })
                            .map(|count| dacode::edits::Applied {
                                in_buffer: count,
                                files: Vec::new(),
                            })
                    });
                }
                dacode_lsp::ServerEvent::CodeActions { actions } => {
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
                dacode_lsp::ServerEvent::References { locations } => {
                    // Into the results panel rather than a list of its own: references are
                    // `path:line` entries you move through and open, which that panel already is.
                    let hits: Vec<dacode_project::SearchHit> = locations
                        .iter()
                        .filter_map(|location| {
                            let path = dacode_lsp::uri_to_path(location.uri.as_str())?;
                            let line = std::fs::read_to_string(&path)
                                .ok()
                                .and_then(|text| {
                                    text.lines()
                                        .nth(location.range.start.line as usize)
                                        .map(str::to_string)
                                })
                                .unwrap_or_default();
                            Some(dacode_project::SearchHit {
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
                        self.state.show_results(dacode::results::Results::from_hits(
                            &self.state.project_root,
                            "references",
                            hits,
                        ));
                    }
                }
                dacode_lsp::ServerEvent::InlayHints { hints } => {
                    self.state.inlay_hints = hints;
                }
                dacode_lsp::ServerEvent::RequestFailed {
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
                    eprintln!("dacode: {message}");
                    self.state.hover = Some(message);
                    self.edit_version = None;
                    self.save_after_format = false;
                }
                dacode_lsp::ServerEvent::Message { text } => eprintln!("dacode: {text}"),
                dacode_lsp::ServerEvent::Stopped => {
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

    /// Whether the idle step still has to be run on a timer, because something it collects cannot
    /// wake the loop itself.
    ///
    /// With a waker that is only the file walk and a streaming search. Every worker — syntax, git,
    /// format, install, the language server, the file watcher and the folder picker — announces its
    /// own result, so an idle editor with a server running sleeps instead of waking ten times a
    /// second to find nothing. Without a waker (headless tests) nobody announces anything, so the
    /// old rules apply: stay awake while any worker still owes an answer.
    ///
    /// Autosave is not here: it waits on a deadline rather than on work, and
    /// [`autosave_due_at`](Self::autosave_due_at) supplies that.
    fn services_busy(&self) -> bool {
        if self.walk.is_some() {
            return true;
        }
        // A streaming search must keep being polled or its hits never appear.
        if self.state.results.as_ref().is_some_and(|r| !r.finished) {
            return true;
        }
        if self
            .state
            .search
            .as_ref()
            .is_some_and(|panel| panel.searching())
        {
            return true;
        }
        // Every worker below announces its own result through the waker, so an editor with a loop
        // to wake has nothing left to poll for — and an idle one with a language server running
        // sleeps instead of waking ten times a second to find nothing. The walk and the search
        // above are the two producers that have no waker.
        if self.waker.is_some() {
            return false;
        }
        // Without a waker (headless tests) nobody would announce anything, so each answer is
        // waited for here.
        //
        // A running shell may print at any moment, and with nothing to wake the loop its output
        // would wait for a key.
        if !self.terminals.is_empty() {
            return true;
        }
        //
        // A check or a download, whose answer would otherwise wait for a key.
        if self.updater.busy() {
            return true;
        }
        //
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
        if self.blame_worker.busy() {
            return true;
        }
        if self.scm.busy() {
            return true;
        }
        // And for an install. It is the longest-running of them by far, and its whole point is to
        // report back — a reply left sitting in the channel would make a minute-long install look
        // like nothing happened.
        if self.installer.busy() {
            return true;
        }
        if let Some(client) = self.active_client() {
            // A live server always warrants a slow poll: replies arrive unprompted.
            return client.state() != dacode_lsp::State::Stopped;
        }
        self.syntax.as_ref().is_some_and(|worker| worker.pending())
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
        offer: dacode_lsp::CompletionItem,
        anchor: dacode_text::CharOffset,
        imports_current: bool,
    ) {
        let caret = self.state.cursor();
        let accepted = dacode::edits::accept_completion(
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
            .mutate(|document| {
                document.apply(accepted.transaction, dacode_buffer::EditKind::Discrete)
            })
            .is_err()
        {
            return;
        }

        // Walk the snippet's tab stops, exactly as an expanded snippet does.
        if let Some(expansion) = accepted.expansion {
            let session = dacode::snippet::Session::new(&expansion, accepted.at);
            let stop = session
                .as_ref()
                .and_then(|session| session.current())
                .unwrap_or((
                    dacode_text::CharOffset(accepted.at.get() + expansion.text.chars().count()),
                    0,
                ));
            let selection = if stop.1 == 0 {
                Selection::caret(stop.0)
            } else {
                Selection::new(stop.0, dacode_text::CharOffset(stop.0.get() + stop.1))
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
    fn jump_to(&mut self, path: std::path::PathBuf, position: dacode_lsp::lsp_types::Position) {
        let already_here = self
            .state
            .file_path
            .as_deref()
            .map(std::path::Path::new)
            .map(dacode_project::canonical)
            == Some(dacode_project::canonical(&path));

        if !already_here {
            self.activate(Target::File(path));
        }

        let offset = dacode_lsp::from_lsp(self.state.document.text(), position);
        self.state.document.set_primary_caret(offset);
        self.state.hover = None;
        self.reveal_cursor();
    }

    /// When autosave will next be due, if it is waiting on the clock.
    ///
    /// The only thing in the idle step that is a *time* rather than an event: nothing happens at
    /// the end of the quiet period, so nothing wakes the loop for it, and a sleeping editor would
    /// save on the next unrelated input instead. `None` once the moment has passed — what is still
    /// dirty then has been tried and refused (it changed on disk), and a deadline in the past would
    /// spin the loop.
    fn autosave_due_at(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        if !self.state.autosave || !self.state.document.is_dirty() || self.state.file_path.is_none()
        {
            return None;
        }
        let due = self.last_edit? + AUTOSAVE_QUIET;
        (due > now).then_some(due)
    }

    /// How the loop sleeps after an idle step: until input or a wake-up, except for the two things
    /// that cannot announce themselves.
    ///
    /// Those are anything [`services_busy`](Self::services_busy) still polls for, which a tick of
    /// 100 ms covers, and autosave's quiet period, which needs a wake-up at its own deadline.
    fn control_flow(&self, now: std::time::Instant) -> ControlFlow {
        let tick = self
            .services_busy()
            .then(|| now + std::time::Duration::from_millis(100));
        match [
            tick,
            self.autosave_due_at(now),
            self.settings_save_due_at(now),
        ]
        .into_iter()
        .flatten()
        .min()
        {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        }
    }

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
        if !self.state.autosave || !self.state.document.is_dirty() {
            return false;
        }
        // A buffer with nowhere to go is left alone rather than prompting: autosave is the setting
        // for people who do not want to be asked, and asking them where to put it is the one thing
        // it must not do.
        if self.state.file_path.is_none() {
            return false;
        }
        if self
            .last_edit
            .is_none_or(|at| at.elapsed() < AUTOSAVE_QUIET)
        {
            return false;
        }

        self.write_active(Saving::Auto);
        true
    }

    /// Keeps whatever the editor is saying for the bell, once.
    ///
    /// Read from the two places messages appear — the hover line and a prompt's message — rather
    /// than from every site that sets one, of which there are some sixty: a log that each of them
    /// had to remember to write to would be missing whichever one was added next.
    fn record_notice(&mut self) {
        let current = self
            .state
            .prompt
            .as_ref()
            .and_then(|prompt| prompt.message.clone())
            .or_else(|| self.state.hover.clone());
        if current == self.last_notice {
            return;
        }
        if let Some(message) = &current {
            if self.server_hover.as_ref() != Some(message) {
                // The first line: a multi-line message is a paragraph, and the panel lists headlines.
                let headline = message.lines().next().unwrap_or_default().to_string();
                self.state.chrome.notify(&headline);
                self.request_redraw();
            }
        }
        self.last_notice = current;
    }

    /// Hands every dirty buffer to the panic hook, if anything about them changed.
    ///
    /// Here, once per batch of events, rather than after each edit: everything that changes text
    /// runs inside an event, and every event is followed by this. A panic inside a batch loses that
    /// batch's keystrokes and nothing older. Bounded by the number of open buffers, never by their
    /// length — a [`Text`] clone shares the rope.
    fn hold_lifeline(&mut self) {
        let now = Held {
            active: self.state.active_buffer,
            buffers: self.buffers.len(),
            version: self.state.document.version(),
            dirty: self.state.document.is_dirty(),
            path: self.state.file_path.clone(),
        };
        if self.held.as_ref() == Some(&now) {
            return;
        }

        let active = (self.state.file_path.as_deref(), &self.state.document);
        let others = self
            .buffers
            .iter()
            .map(|buffer| (buffer.file_path.as_deref(), &buffer.document));
        // A scratch buffer has no path to recover to, so it is not held — the same limitation the
        // timed write has.
        let dirty = std::iter::once(active)
            .chain(others)
            .filter(|(_, document)| document.is_dirty())
            .filter_map(|(path, document)| {
                Some((std::path::PathBuf::from(path?), document.text().clone()))
            })
            .collect();
        self.lifeline.hold(self.recovery.directory(), dirty);
        self.held = Some(now);
    }

    /// Writes a recovery copy of the active buffer if it is dirty.
    ///
    /// Throttled by `recovery::INTERVAL`, so this is safe to call on every keystroke; `force` is for
    /// the moments where losing anything is unacceptable.
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
            let _ = dacode::recovery::write_in(
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
            "dacode: recovered unsaved changes for {path} from {} — the buffer is dirty; {} keeps them",
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
            self.watch_project_directories();
            eprintln!(
                "dacode: indexed {count} files{}",
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
        self.requested_range = Some(range);
        self.collect_syntax();
    }

    /// The one idle step for every background worker: syntax, language server, formatter, installer
    /// and git. Returns whether anything on screen changed.
    ///
    /// Called from `user_event`, which is how a worker's wake-up is answered, and from
    /// `about_to_wait`. One function so that the two cannot disagree about which workers there are —
    /// a worker added to one list and not the other is a result that arrives on a keypress.
    fn collect_background(&mut self) -> bool {
        let mut changed = self.collect_syntax();
        changed |= self.sync_lsp();
        changed |= self.poll_formatter();
        changed |= self.poll_installer();
        changed |= self.sync_git();
        changed |= self.sync_blame();
        changed |= self.sync_scm();
        changed |= self.poll_terminal();
        changed |= self.poll_updates();
        changed |= self.sync_preview();
        changed
    }

    /// The focused pane's whole slot: where a viewed picture is drawn. The rectangle `draw_panes`
    /// gives `draw_preview`, from the same `Layout::pane`.
    fn preview_slot(&self) -> Rect {
        let count = self.panes.len().max(1);
        let vertical = self.panes.layout == chrome::PaneLayout::Vertical;
        self.layout()
            .pane(self.panes.active(), count, vertical, self.scale())
            .slot
    }

    /// The viewer's part of the idle step: takes what its worker finished, and asks it for the
    /// pages now on screen at the width they are drawn at. Returns whether there is something new
    /// to draw.
    fn sync_preview(&mut self) -> bool {
        let slot = self.preview_slot();
        let margin = dacode::layout::viewer_margin(self.scale());
        let Some(preview) = &mut self.state.preview else {
            return false;
        };
        let changed = preview.poll();
        let placed = preview.placed(slot, margin);
        preview.request(&placed);
        changed
    }

    /// A key on a viewed picture: zoom and scroll, the keys an image viewer and a PDF reader share.
    /// Returns whether it was used; everything else — closing the tab, the palette — passes on.
    fn handle_preview_key(&mut self, key: &Key) -> bool {
        if key.ctrl || key.alt || self.state.focus != dacode::focus::Focus::Editor {
            return false;
        }
        let slot = self.preview_slot();
        let scale = self.scale();
        let margin = dacode::layout::viewer_margin(scale);
        let row = 48.0 * scale;
        let Some(preview) = self
            .state
            .preview
            .as_mut()
            .filter(|preview| preview.kind.is_drawn())
        else {
            return false;
        };
        let page = slot.height - margin * 2.0;
        match key.code {
            KeyCode::Char('+') | KeyCode::Char('=') => preview.zoom_by(1.25),
            KeyCode::Char('-') => preview.zoom_by(0.8),
            KeyCode::Char('0') => preview.fit_view(),
            KeyCode::Down | KeyCode::Char('j') => preview.scroll_by(row, 0.0, slot, margin),
            KeyCode::Up | KeyCode::Char('k') => preview.scroll_by(-row, 0.0, slot, margin),
            KeyCode::Right | KeyCode::Char('l') => preview.scroll_by(0.0, row, slot, margin),
            KeyCode::Left | KeyCode::Char('h') => preview.scroll_by(0.0, -row, slot, margin),
            KeyCode::PageDown | KeyCode::Char(' ') => preview.scroll_by(page, 0.0, slot, margin),
            KeyCode::PageUp => preview.scroll_by(-page, 0.0, slot, margin),
            KeyCode::Home | KeyCode::Char('g') => preview.scroll_by(f32::MIN, 0.0, slot, margin),
            KeyCode::End | KeyCode::Char('G') => preview.scroll_by(f32::MAX, 0.0, slot, margin),
            _ => return false,
        }
        self.sync_preview();
        true
    }

    /// The wheel over a viewed picture: scrolls it, or with ctrl held zooms it. Returns whether
    /// there was a picture to move.
    fn scroll_preview(&mut self, dx: f32, dy: f32, zoom: bool) -> bool {
        let slot = self.preview_slot();
        let margin = dacode::layout::viewer_margin(self.scale());
        let Some(preview) = self
            .state
            .preview
            .as_mut()
            .filter(|preview| preview.kind.is_drawn())
        else {
            return false;
        };
        if zoom {
            preview.zoom_by(if dy < 0.0 { 1.1 } else { 1.0 / 1.1 });
        } else {
            preview.scroll_by(dy, dx, slot, margin);
        }
        self.sync_preview();
        true
    }

    /// Takes whatever the highlighter has finished and applies it if it still describes the buffer.
    /// Returns whether anything on screen changed.
    ///
    /// Called after each request and from the idle step, so a parse that finishes while the editor
    /// is asleep is applied when the waker rouses it. Comparing before replacing is what keeps an
    /// idle poll from asking for a redraw on every wake-up.
    fn collect_syntax(&mut self) -> bool {
        let Some(worker) = self.syntax.as_mut() else {
            return false;
        };
        // Apply only what is fresh. A stale result is kept for the ledger to report, never
        // painted over text that has since moved.
        worker.poll();
        let version = worker.version().unwrap_or(Version(0));
        let mut changed = version != self.state.syntax_version;
        self.state.syntax_version = version;

        let buffer = self.state.document.version();
        if let Some(result) = worker.fresh_for(buffer) {
            // The tree changes nothing on screen, so it is taken without asking for a redraw —
            // and taken even when the spans are unchanged, because a keystroke that leaves the
            // colours alone still leaves the old tree describing text that has moved.
            if self
                .state
                .structure
                .as_ref()
                .is_none_or(|held| held.version() != result.version)
            {
                self.state.structure = result.structure.clone();
            }
            if result.spans != self.state.highlights
                || result.node_path != self.state.node_path
                || result.has_error != self.state.syntax_error
            {
                self.state.highlights = result.spans.clone();
                self.state.node_path = result.node_path.clone();
                self.state.syntax_error = result.has_error;
                changed = true;
            }
        }
        changed
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
                if prompt.is_replace() && prompt.focus == dacode::prompt::Field::Replacement =>
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
                    PromptKind::NewBranch => {
                        let name = prompt.input.trim().to_string();
                        if name.is_empty() {
                            self.warn("a branch needs a name");
                        } else {
                            self.run_git(dacode::scm::Op::CreateBranch(name));
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
                            // A diff saved as a file is a patch, and a file is editable.
                            self.state.review = None;
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
                self.state.open_search(&query);
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

        let Some(target) = dacode::edits::target_at(&matches, self.state.cursor()) else {
            return;
        };
        let at = target.start;
        match self.state.mutate(|document| {
            dacode::edits::replace_matches(document, std::slice::from_ref(&target), &replacement)
        }) {
            Ok(_) => {
                // Where the caret lands: after what was just written, so the next `↵` finds the
                // *next* match rather than re-matching what the replacement happens to contain.
                self.state
                    .document
                    .set_primary_caret(dacode_text::CharOffset(
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
            .mutate(|document| dacode::edits::replace_matches(document, &matches, &replacement))
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
        if let Some(target) = dacode_buffer::find_next(&matches, from, forward) {
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

        let matches = dacode_buffer::find_all(
            self.state.document.text(),
            &query,
            &dacode_buffer::SearchOptions::default(),
        );

        // Show the search as it is typed: the highlight is the feedback that makes `/` feel live.
        self.state.search_matches = matches.clone();
        if let Some(found) = dacode_buffer::find_next(&matches, origin, forward) {
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
                    self.state.review = None;
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
                let offset = text.point_to_char(dacode_text::Point::new(target, 0));
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
                    self.state.show_results(dacode::results::Results::start(
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
                    preview: state.preview.as_ref(),
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
                        preview: buffer.preview.as_ref(),
                    },
                    // The buffer went away — a pane pointing at nothing draws the live one
                    // rather than vanishing.
                    None => chrome::PaneView {
                        document: &state.document,
                        scroll: pane.scroll,
                        path: &state.path,
                        focused,
                        highlights: &state.highlights,
                        preview: state.preview.as_ref(),
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
fn load_snippets() -> Vec<dacode_import::Snippet> {
    let directory = dacode::config::config_dir().join("snippets");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };

    let mut snippets = Vec::new();
    let mut report = dacode_import::ImportReport::default();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| dacode_import::parse_jsonc(&text).ok())
        {
            Some(json) => snippets.extend(dacode_import::import_snippets(&json, &mut report)),
            None => eprintln!("dacode: could not read snippets from {}", path.display()),
        }
    }

    if !snippets.is_empty() {
        eprintln!(
            "dacode: {} snippet{} loaded",
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
/// Who asked for a save, which decides what it may overwrite and how a refusal is said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Saving {
    /// A key, a menu, `:w`: someone chose to.
    Asked,
    /// Autosave, after typing paused.
    Auto,
    /// The write that follows a format, which must not arm another.
    AfterFormat,
}

/// What the panic lifeline was last filled from. Background buffers cannot change without the
/// active one changing places or the list changing length, so this is enough to know when it is
/// stale.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    active: usize,
    buffers: usize,
    version: dacode_text::Version,
    dirty: bool,
    path: Option<String>,
}

/// Whether a stored buffer is the empty scratch buffer the editor starts with and nothing more:
/// no file, no text, nothing to lose by dropping it.
/// The tab name of a read-only tab: the file and which of its changes, or the history.
fn review_crumbs(show: &dacode::scm::Show) -> Vec<String> {
    match show {
        dacode::scm::Show::Changes { path, side } => {
            let mut crumbs: Vec<String> = path.split('/').map(str::to_string).collect();
            if let Some(last) = crumbs.last_mut() {
                *last = format!("{last} ({})", side.describe());
            }
            crumbs
        }
        dacode::scm::Show::History => vec!["git".to_string(), "history".to_string()],
    }
}

fn is_placeholder(buffer: &Buffer) -> bool {
    buffer.file_path.is_none()
        && !buffer.document.is_dirty()
        && buffer.document.text().len_chars() == dacode_text::CharOffset(0)
}

/// How the running copy was installed, from where its executable is.
fn this_install() -> dacode::update::Install {
    let Ok(exe) = std::env::current_exe() else {
        return dacode::update::Install::Unknown;
    };
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    dacode::update::install_kind(
        &exe,
        cfg!(windows),
        &dacode::paths::home(),
        local.as_deref(),
    )
}

/// The machine's package format: `/etc/os-release`, then whichever package tool is installed.
fn this_package() -> Option<dacode::update::Package> {
    let release = std::fs::read_to_string("/etc/os-release").ok();
    dacode::update::package_family(release.as_deref(), |tool| {
        dacode_lsp::resolve(tool).is_some()
    })
}

fn buffer_key(file_path: Option<&str>) -> Option<std::path::PathBuf> {
    file_path.map(|path| dacode_project::canonical(std::path::Path::new(path)))
}

/// A path typed into one of the editor's own prompts, made absolute against the project root.
///
/// A windowed editor's working directory is wherever the desktop launched it from, and never what
/// someone typing `src/notes.rs` into Save As means. Both `fs::write` and
/// [`dacode_project::canonical`] resolve a relative path against that directory, so leaving it
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
            Action::CheckForUpdates
            | Action::DownloadUpdate
            | Action::RestartToUpdate
            | Action::UpdateReleaseNotes
            | Action::SkipUpdate => {
                self.update_action(action);
                return;
            }
            Action::ShowSourceControl => {
                // A second press closes it, as the activity icons do in every editor that has them.
                if self.state.source_control_showing() {
                    self.state.close_sidebar();
                } else {
                    self.state.open_source_control();
                    self.refresh_scm(false);
                }
                return;
            }
            Action::Git(git) => {
                self.git_action(*git);
                return;
            }
            _ => {}
        }
        match action {
            // File ▸ Save and the palette reach here without the settings tab's ctrl-s intercept,
            // and would save — and maybe reformat — the file hidden behind the tab.
            Action::Save if self.state.settings_showing() => {
                self.state.hover =
                    Some("settings save themselves a moment after each change".to_string());
            }
            Action::Save => self.save_active(),
            Action::Share => {
                let Some(path) = self.state.file_path.clone() else {
                    self.warn("nothing to share — this buffer has no file");
                    return;
                };
                let line = self.state.cursor_point().line + 1;
                let root = self.state.project_root.clone();
                let link = dacode::share::link(&root, std::path::Path::new(&path), line);
                dacode::clipboard::set(&link);
                self.state.hover = Some(format!("copied {link}"));
            }
            Action::PreviewFile => {
                // Only an SVG goes back to its text: every other viewed file has no text to show,
                // and an office document's is already what is on screen.
                match self.state.preview.as_ref().map(|preview| preview.kind) {
                    Some(dacode::viewer::Kind::Svg) => self.state.preview = None,
                    Some(_) => {}
                    None if self.state.document.is_dirty() => {
                        self.warn(
                            "save the file first — the picture is drawn from what is on disk",
                        );
                    }
                    None => {
                        self.ensure_preview(true);
                        if self.state.preview.is_none() {
                            self.warn("this file is not something DaCode can show as a picture");
                        }
                    }
                }
            }
            Action::Run => {
                // Typed into the terminal and entered, as if you had: the command stays in the
                // shell's history, its output scrolls where you can read it, and ctrl-c stops it.
                let root = self.state.project_root.clone();
                // A file path may be relative to the project; the shell gets it whole, because
                // the person may have `cd`'d somewhere else in it since.
                let file = self
                    .state
                    .file_path
                    .as_deref()
                    .filter(|_| self.state.review.is_none())
                    .map(|path| std::path::PathBuf::from(absolute_in(&root, path)));
                if file.is_some() && self.state.document.is_dirty() {
                    // What runs is what is on disk, so what is on screen goes there first. As
                    // autosave does, which never overwrites a file changed from outside.
                    self.write_active(Saving::Auto);
                }
                match dacode::share::run_command_for(&root, file.as_deref(), cfg!(windows)) {
                    Some(command) => {
                        if self.show_terminal(true) {
                            if let Some(terminal) = self.active_terminal() {
                                terminal.write(format!("{command}\r").into_bytes());
                            }
                        }
                    }
                    None => {
                        self.state.hover = Some(
                            "nothing here DaCode knows how to run — the terminal is open to type \
                             a command"
                                .to_string(),
                        );
                        self.show_terminal(true);
                    }
                }
            }
            Action::ToggleTerminal => self.toggle_terminal(),
            Action::FocusTerminal => {
                self.show_terminal(true);
            }
            Action::NewTerminal => {
                // Shown first, so the new shell starts at the size of the panel it will be drawn
                // in; the shells already running keep running in their own tabs.
                let was_open = self.state.terminal.open;
                self.state.terminal.open = true;
                self.state.focus = dacode::focus::Focus::Terminal;
                if !self.spawn_terminal() && self.terminals.is_empty() {
                    self.state.terminal.open = was_open;
                    self.state.focus = dacode::focus::Focus::Editor;
                }
            }
            Action::NextTerminal => self.step_terminal(1),
            Action::PreviousTerminal => self.step_terminal(-1),
            Action::KillTerminal => self.kill_terminal(self.state.terminal.active),
            Action::TerminalCopy => self.copy_terminal_selection(),
            Action::TerminalPaste => self.paste_into_terminal(),
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
                let mut prompt =
                    dacode::prompt::Prompt::new(PromptKind::NewFile, self.state.cursor());
                prompt.input = self.tree_directory();
                self.state.prompt = Some(prompt);
            }
            Action::RenameFile => match self.tree_target() {
                Some(relative) => {
                    let mut prompt =
                        dacode::prompt::Prompt::new(PromptKind::RenameFile, self.state.cursor());
                    prompt.input = relative;
                    self.state.prompt = Some(prompt);
                }
                None => self.warn("no file selected"),
            },
            Action::DeleteFile => self.request_delete_file(),
            Action::InstallLanguageServer => self.request_install_server(),
            Action::ExpandSnippet => self.expand_snippet(),
            // The three ways of finding out what the editor can do. Each is a toggle, because the
            // key that opened it is the key someone presses again to get rid of it.
            Action::ToggleCommandPalette => {
                if self.state.palette.is_some() {
                    self.state.close_palette();
                } else {
                    self.open_palette(PaletteKind::Commands);
                }
            }
            Action::ToggleHelp => {
                if self.state.help.is_some() {
                    self.state.close_help();
                } else {
                    self.state.help = Some(dacode::help::Help::new(self.state.modal));
                }
            }
            Action::ToggleMenu => {
                if self.state.menu.is_some() {
                    self.state.close_menu();
                } else {
                    self.state
                        .open_menu(dacode::menu::Menu::bar(self.state.modal));
                }
            }
            Action::ToggleContextMenu => self.open_context_menu(),
            Action::OpenFolder => {
                // The desktop's own folder chooser when there is one — that is what "open a folder"
                // means to everyone who did not come from Vim. Typing a path is the fallback, not the
                // interface.
                if dacode::picker::is_available() {
                    if self.folder_picker.is_none() {
                        let start = self
                            .state
                            .project_root
                            .parent()
                            .unwrap_or(&self.state.project_root)
                            .to_path_buf();
                        self.folder_picker = Some(dacode::picker::FolderPicker::open(
                            start,
                            self.waker.clone(),
                        ));
                    }
                } else {
                    let mut prompt =
                        dacode::prompt::Prompt::new(PromptKind::OpenFolder, self.state.cursor());
                    // Pre-filled with the current root and a separator, so the common case — a
                    // sibling project — is a few characters rather than a whole path.
                    prompt.input = format!("{}/", self.state.project_root.display());
                    self.state.prompt = Some(prompt);
                }
            }
            Action::ToggleSettings => self.open_settings_tab(),
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
            Action::ShowCommit => self.show_commit(self.state.cursor_point().line),
            Action::ToggleTree => {
                if self.state.tree.is_some() {
                    self.state.tree = None;
                    self.state.focus = dacode::focus::Focus::Editor;
                } else {
                    let mut tree = dacode::tree::FileTree::new(
                        self.index.root().to_path_buf(),
                        self.index.files(),
                    );
                    // Open onto the file being edited rather than a collapsed root: the tree is a
                    // way to look around from where you are, not a fresh start.
                    if let Some(path) = &self.state.file_path {
                        let absolute = dacode_project::canonical(std::path::Path::new(path));
                        if let Ok(relative) = absolute.strip_prefix(self.index.root()) {
                            tree.reveal(&relative.to_string_lossy());
                        }
                    }
                    self.state.tree = Some(tree);
                    self.state.show_diagnostics = false;
                    self.state.focus = dacode::focus::Focus::Sidebar;
                }
            }
            // Only reached with the tree closed (the executor handles the rest): open it, which
            // gives it the keys, rather than leaving a keystroke that appears to do nothing.
            Action::FocusExplorer => self.show_tree(),
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
        let contents = match dacode::open::read(path) {
            dacode::open::Opened::Text(contents) => contents,
            // Restored as an empty buffer with the file's name; the viewer starts when it is shown.
            dacode::open::Opened::View(_) => String::new(),
            dacode::open::Opened::Refused(why) => {
                eprintln!("dacode: not restoring {}: {why}", path.display());
                return;
            }
        };
        self.remember_disk(&path.to_string_lossy());

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
            .point_to_char(dacode_text::Point::new(line, column));
        document.set_primary_caret(offset);

        let display = path.to_string_lossy().to_string();
        self.buffers.push(Buffer {
            document,
            path: relative_crumbs(&display),
            file_path: Some(display),
            scroll: ScrollAnchor::default(),
            highlights: Vec::new(),
            review: None,
            preview: None,
        });
        self.state.buffer_count = self.buffers.len() + 1;
    }

    /// Puts the cursor back where the session left it.
    fn restore_cursor(&mut self, line: usize, column: usize) {
        let text = self.state.document.text();
        let line = line.min(text.last_line());
        let offset = text.point_to_char(dacode_text::Point::new(line, column));
        self.state.document.set_primary_caret(offset);
    }

    /// Records the open files and where the cursor is in each, for the next run.
    ///
    /// Written on the way out rather than continuously: unlike crash recovery, which must survive a
    /// kill, a session is only interesting when the editor closed properly.
    fn save_session(&mut self) {
        self.save_pane();
        let session = self.session_snapshot();
        dacode::session::save(&dacode::session::session_dir(), self.index.root(), &session);
    }

    /// Reopens the rest of the last session, behind the file already live.
    ///
    /// A method rather than a block inside `main` so a test can drive the real one. Written as a
    /// loop inside `main`, it was untestable, and the test that would have caught the ordering
    /// defect below had to keep its own copy of the walk — which passes whatever `main` does.
    fn restore_session(&mut self, session: &dacode::session::Session) {
        if let Some(fraction) = session.terminal_fraction {
            self.state.terminal.fraction = fraction;
        }
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
            eprintln!("dacode: restored {others} more from the last session");
        }
    }

    /// What the session file would say. Separate from writing it so the buffer walk — which is
    /// where both of this function's defects lived — can be tested without a window or a disk.
    fn session_snapshot(&self) -> dacode::session::Session {
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
            files.push(dacode::session::OpenFile {
                path: std::path::PathBuf::from(path),
                line: point.line,
                column: point.column,
            });
        }

        dacode::session::Session {
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
            // Only once dragged, so a default that changes later reaches someone who never did.
            terminal_fraction: Some(self.state.terminal.fraction).filter(|fraction| {
                (fraction - dacode::terminal::DEFAULT_FRACTION).abs() > f32::EPSILON
            }),
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
                    operator: dacode_input::Operator::Indent,
                    target: dacode_input::Target::Selection,
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
            text.point_to_char(dacode_text::Point::new(text.char_to_point(cursor).line, 0));
        let before = text.slice_to_string(line_start..cursor);

        let Some(snippet) = dacode::snippet::trigger(&before, &self.snippets).cloned() else {
            self.state.mutate(|document| document.insert("    "));
            return;
        };

        let indent: String = before.chars().take_while(|c| c.is_whitespace()).collect();
        let expansion = dacode::snippet::expand(&snippet.body, &indent);

        // The prefix the user typed is replaced by the expansion, as one edit — so `tab` undoes in
        // one press rather than leaving half a snippet behind.
        let prefix_len = snippet.prefix.chars().count();
        let start = CharOffset(cursor.get().saturating_sub(prefix_len));
        let transaction = dacode_text::Transaction::new(vec![dacode_text::Edit::replace(
            start..cursor,
            expansion.text.clone(),
        )]);
        if self
            .state
            .mutate(|document| document.apply(transaction, dacode_buffer::EditKind::Discrete))
            .is_err()
        {
            return;
        }

        let session = dacode::snippet::Session::new(&expansion, start);
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
    fn here(&self) -> dacode_input::Jump {
        let point = self.state.cursor_point();
        dacode_input::Jump {
            path: self.state.file_path.clone(),
            line: point.line,
            column: point.column,
        }
    }

    /// Goes to a remembered position, opening its file if it is not the one on screen.
    fn go_to(&mut self, jump: dacode_input::Jump) {
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
        let offset = text.point_to_char(dacode_text::Point::new(line, jump.column));
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
        // The remainder was the last pane's. Left until the next event, a click that switched panes
        // would measure the pane it landed in — drawn on whole rows — against it, a row low.
        self.settle_scroll_input();
    }

    /// Sends a rename request for the symbol under the cursor.
    fn request_rename(&mut self, new_name: &str) {
        let Some(path) = self.state.file_path.clone() else {
            self.warn("rename needs a saved file");
            return;
        };
        let ready = self
            .active_client()
            .map(|client| client.state() == dacode_lsp::State::Ready);
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

        let position = dacode_lsp::to_lsp(self.state.document.text(), self.state.cursor());
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
            Option<dacode_text::Version>,
        ) -> Result<dacode::edits::Applied, dacode::edits::EditError>,
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
                    self.write_active(Saving::AfterFormat);
                }
                if !applied.files.is_empty() {
                    let names: Vec<String> = applied
                        .files
                        .iter()
                        .map(|(path, count)| format!("{} ({count})", path.display()))
                        .collect();
                    eprintln!("dacode: {what} — also wrote {}", names.join(", "));
                }
                self.reveal_cursor();
            }
            Err(error) => {
                self.save_after_format = false;
                eprintln!("dacode: {what} failed: {error}");
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
            dacode::edits::replace_in_hits(document, path.as_deref(), &hits, pattern, replacement)
        }) {
            Ok(applied) if applied.total() == 0 => self.warn("nothing matched"),
            Ok(applied) => {
                let summary = applied.summary(&format!("replaced `{pattern}`"));
                eprintln!("dacode: {summary}");
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
        let mut prompt = dacode::prompt::Prompt::new(PromptKind::Command, self.state.cursor());
        prompt.message = Some(message.to_string());
        self.state.prompt = Some(prompt);
    }

    /// The key that runs a command *in the grammar the user is actually using*.
    ///
    /// For the messages that told people what to press next. Several of them named ex-commands —
    /// `:q!`, `:wq`, `:w <path>` — to everyone, including the people who had turned Vim off and
    /// whose grammar cannot parse the leading `:`. Read from the command table, so a message cannot
    /// drift from the binding it is teaching.
    fn key_for(&self, command: &str) -> String {
        dacode_input::commands::by_name(command)
            .map(|command| command.shown(self.state.modal))
            .unwrap_or_default()
    }

    /// How to run a command, as a phrase that can go in the middle of a sentence.
    fn how_to_run(&self, command: &str) -> String {
        how_to_run(command, self.state.modal)
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
        let geometry = dacode::welcome::geometry(welcome, bounds, scale, &self.state.typography);

        if let Some(index) = geometry
            .options
            .iter()
            .position(|option| dacode::layout::contains(*option, x, y))
        {
            welcome.selected = index;
            // On the theme row the highlight *is* the answer, so a click has to preview it too.
            if welcome.step == dacode::welcome::Step::Theme {
                welcome.answers.theme = dacode_theme::Theme::grouped()[index];
            }
            welcome.choose();
            if welcome.is_done() {
                self.finish_welcome();
            }
            return true;
        }

        if geometry
            .next
            .is_some_and(|next| dacode::layout::contains(next, x, y))
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
        let (width, height) = self.window_size();
        Rect::new(0.0, 0.0, width as f32, height as f32)
    }

    fn window_size(&self) -> (u32, u32) {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.size())
            .unwrap_or(self.headless_size)
    }

    fn handle_welcome_key(&mut self, key: Key) -> bool {
        // The theme cards are a grid, so ↑ and ↓ move by a row of it: its columns, from the same
        // layout the cards are drawn and clicked from.
        let columns = dacode::welcome::theme_columns(
            self.window_bounds(),
            self.scale(),
            &self.state.typography,
        );
        let Some(welcome) = &mut self.state.welcome else {
            return false;
        };
        let grid = welcome.step == dacode::welcome::Step::Theme;

        match key.code {
            KeyCode::Char('j') | KeyCode::Down if grid => welcome.move_row(1, columns),
            KeyCode::Char('k') | KeyCode::Up if grid => welcome.move_row(-1, columns),
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Right => welcome.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Left
                if key.code == KeyCode::Left && welcome.step != dacode::welcome::Step::Theme =>
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

        let directory = self.config_dir.clone();
        let path = directory.join("init.lua");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let written = dacode::welcome::config_from(&answers, &existing);

        if std::fs::create_dir_all(&directory).is_ok() {
            self.self_written.insert(dacode_project::canonical(&path));
            match dacode::save::write(&path, written.as_bytes()) {
                Ok(()) => {
                    self.self_written.insert(dacode_project::canonical(&path));
                    eprintln!("dacode: wrote {}", path.display());
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
                .fold(dacode_import::Checklist::nothing(), |list, item| {
                    list.with(item.item())
                });
            match dacode::import_cli::import_from_installed_vscode(checklist, &directory) {
                Ok(report) => {
                    let rendered = report.render();
                    eprintln!("{rendered}");
                    self.reload_config();
                    self.state.hover = Some(rendered);
                }
                Err(error) => self.state.hover = Some(format!("import failed: {error}")),
            }
        }
    }

    /// Keys for the settings tab, while it is the tab on screen and has the keys. Returns true if
    /// the key was used; a key with ctrl or alt, or a function key, never is, so the palette, the
    /// menus, `ctrl-w` and F1 work over the page as they do over the text.
    ///
    /// ↑ ↓ choose a row (page up and down, home and end by more), ← → change it, space and enter do
    /// what a click on it does, and delete puts a changed setting back to its default. Every other
    /// printable key is typed into the search box and backspace takes one back; escape clears the
    /// search, then leaves the tab for the file under it. While a search is being typed, space is
    /// part of it — "line numbers" needs one — and ← → and enter still change the selected row.
    fn handle_page_key(&mut self, key: Key) -> bool {
        use dacode::settings_ui::{Entry, Line};
        // `ctrl-f` finds — and on this page typing already goes to the search box, so it is taken
        // here rather than opening a find prompt over the file behind the tab.
        if key.ctrl && !key.shift && !key.alt && key.code == KeyCode::Char('f') {
            return true;
        }
        // The old panel taught ctrl-s as "write settings". Here it would save — and maybe
        // reformat — the file hidden behind the tab, so it answers the habit instead.
        if key.ctrl && !key.shift && !key.alt && key.code == KeyCode::Char('s') {
            self.state.hover =
                Some("settings save themselves a moment after each change".to_string());
            return true;
        }
        if key.ctrl || key.alt {
            return false;
        }
        let modal = self.state.modal;
        let Some(page) = &self.state.settings_page else {
            return false;
        };
        let entry = page.selected_entry(modal);
        let query = page.query().to_string();
        match key.code {
            KeyCode::Up => self.move_page_selection(-1),
            KeyCode::Down => self.move_page_selection(1),
            KeyCode::PageUp => self.move_page_selection(-8),
            KeyCode::PageDown => self.move_page_selection(8),
            KeyCode::Home => self.move_page_selection(isize::MIN / 2),
            KeyCode::End => self.move_page_selection(isize::MAX / 2),
            KeyCode::Left | KeyCode::Right => {
                let delta = if key.code == KeyCode::Left { -1 } else { 1 };
                match entry {
                    Some(Entry::Field(field)) => self.adjust_setting(field, delta),
                    Some(Entry::Line(Line::Import(item))) => self.tick_import(item),
                    // An arrow changes a value; it does not start an import that writes files.
                    _ => {}
                }
            }
            KeyCode::Char(' ') if query.is_empty() => {
                if let Some(entry) = entry {
                    self.choose_page_entry(entry);
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = entry {
                    self.choose_page_entry(entry);
                }
            }
            KeyCode::Delete => match entry {
                Some(Entry::Field(field)) => self.reset_setting(field),
                Some(Entry::Command(command)) => self.restore_keys(command),
                _ => {}
            },
            KeyCode::Backspace => {
                let mut shorter = query;
                if shorter.pop().is_some() {
                    self.set_settings_query(shorter);
                }
            }
            KeyCode::Escape if !query.is_empty() => self.set_settings_query(String::new()),
            // Back to the file, which is still where it was; the tab stays open beside it.
            KeyCode::Escape => self.hide_settings_tab(),
            KeyCode::Char(typed) => {
                let mut longer = query;
                longer.push(typed);
                self.set_settings_query(longer);
            }
            _ => return false,
        }
        true
    }

    /// Moves the page's selection and scrolls to keep it whole on screen.
    fn move_page_selection(&mut self, delta: isize) {
        let (rect, scale, modal) = (self.layout().editor, self.scale(), self.state.modal);
        let typography = self.state.typography;
        if let Some(page) = &mut self.state.settings_page {
            page.move_selection(delta, modal);
            page.reveal(rect, scale, modal, &typography);
        }
    }

    /// A new search: the list starts again from its top, at once rather than gliding there.
    fn set_settings_query(&mut self, query: String) {
        let modal = self.state.modal;
        if let Some(page) = &mut self.state.settings_page {
            page.set_query(query, modal);
        }
        self.state.snap_settings_scroll();
    }

    /// Opens a page of the tab from the tree. Choosing a page ends a search, whose list was every
    /// page's; the new page fades in over the old one rather than replacing it between two frames.
    fn open_settings_category(&mut self, category: dacode::settings_ui::Category) {
        let modal = self.state.modal;
        if let Some(page) = &mut self.state.settings_page {
            page.set_query(String::new(), modal);
            page.set_category(category, modal);
        }
        self.state.snap_settings_scroll();
        let now = std::time::Instant::now();
        let motion = &mut self.state.chrome.motion;
        motion.snap(dacode::motion::Key::SettingsCategory, 0.0);
        motion.to(
            dacode::motion::Key::SettingsCategory,
            1.0,
            dacode::motion::Spring {
                damping_ratio: 1.0,
                ..dacode::motion::Spring::PANEL
            },
            now,
        );
    }

    /// What choosing a row does, by enter, space or a click on it: the same thing each way.
    fn choose_page_entry(&mut self, entry: dacode::settings_ui::Entry) {
        use dacode::settings_ui::{Entry, Line, Link};
        match entry {
            Entry::Heading { .. } => {}
            Entry::Command(command) => self.start_key_capture(command),
            // Thirteen themes are too many to step through one click at a time: the row opens the
            // list of them instead.
            Entry::Field(dacode::settings_ui::Field::Theme) => self.open_theme_dropdown(),
            Entry::Field(field) => self.cycle_setting(field),
            Entry::Line(Line::Import(item)) => self.tick_import(item),
            Entry::Line(Line::RunImport) => self.run_import(),
            Entry::Line(Line::LoadTheme) => self.prompt_for_theme_file(),
            // A report, not a control — so choosing it does the one useful thing a row cannot:
            // shows the whole line, which for a language server is a path that rarely fits.
            Entry::Line(line @ (Line::Server(at) | Line::Formatter(at))) => {
                let report = self.state.settings_page.as_ref().and_then(|page| {
                    if matches!(line, Line::Formatter(_)) {
                        page.formatters().get(at)
                    } else {
                        page.servers().get(at)
                    }
                });
                // A missing one DaCode can download is offered, armed by this choice and run by the
                // next — the mouse's way to what `install_language_server` does from the keys.
                let offer = report
                    .filter(|report| !report.found && !report.configured)
                    .and_then(|report| {
                        dacode::tools::TOOLS
                            .iter()
                            .find(|tool| tool.provides.contains(&report.program.as_str()))
                            .map(|tool| (tool, report.program.clone(), report.language.clone()))
                    })
                    .filter(|(tool, _, _)| {
                        dacode::tools::Platform::current()
                            .is_some_and(|platform| dacode::tools::available(tool, platform))
                    });
                if let Some((tool, program, language)) = offer {
                    self.offer_download(tool, program, language);
                } else if let Some(report) = report {
                    self.state.hover = Some(format!(
                        "{} — {} · {}",
                        report.language, report.program, report.status
                    ));
                }
            }
            Entry::Link(Link::RebindKeys) => self.open_init_lua(),
            Entry::Link(Link::ResetKeys) => {
                if dacode_input::shortcuts::current().is_empty() {
                    self.state.hover = Some("every key is already the built-in".to_string());
                } else {
                    self.change_shortcuts(dacode_input::Shortcuts::clear);
                    self.state.hover = Some(Link::ResetKeys.explanation().to_string());
                }
            }
        }
    }

    /// Whether a Keys page row is waiting for a key — and is on screen with the keys, so a hidden
    /// tab cannot eat them.
    fn capturing_key(&self) -> bool {
        self.state.settings_showing()
            && self.state.focus == dacode::focus::Focus::Editor
            && self
                .state
                .settings_page
                .as_ref()
                .is_some_and(|page| page.capture().is_some())
    }

    /// A click or Enter on a command row: it waits for its new key.
    fn start_key_capture(&mut self, command: &'static dacode_input::Command) {
        if let Some(page) = &mut self.state.settings_page {
            page.start_capture(command);
        }
        self.state.pending.clear();
    }

    /// A key pressed while a command row waits for one.
    fn handle_capture_key(&mut self, key: Key) {
        use dacode::settings_ui::CaptureStep;
        let modal = self.state.modal;
        let Some(step) = self
            .state
            .settings_page
            .as_mut()
            .and_then(|page| page.capture_key(key, modal))
        else {
            return;
        };
        let say = |keys: &str| {
            if modal {
                keys.to_string()
            } else {
                dacode_input::commands::pretty(keys)
            }
        };
        match step {
            CaptureStep::Waiting | CaptureStep::Cancelled => {}
            CaptureStep::NotAShortcut => {
                self.state.hover = Some(
                    "a shortcut starts with ctrl, alt or a function key — Esc cancels".to_string(),
                );
            }
            CaptureStep::Unbind(command) => {
                self.change_shortcuts(|shortcuts| shortcuts.unbind(command));
                self.state.hover = Some(format!("\u{201c}{}\u{201d} has no key now", command.what));
            }
            CaptureStep::Assign(command, keys) => {
                let written = dacode_input::shortcuts::notation(&keys);
                self.change_shortcuts(|shortcuts| shortcuts.assign(command, keys));
                self.state.hover = Some(format!(
                    "{} now runs \u{201c}{}\u{201d}",
                    say(&written),
                    command.what
                ));
            }
        }
    }

    /// Changes the keys commands have, at once — the grammar and every surface that prints a key
    /// read the installed set — and has the keys block of `init.lua` rewritten after the same pause
    /// a setting waits.
    fn change_shortcuts(&mut self, change: impl FnOnce(&mut dacode_input::Shortcuts)) {
        let mut shortcuts = (*dacode_input::shortcuts::current()).clone();
        change(&mut shortcuts);
        dacode_input::shortcuts::install(shortcuts);
        self.keys_touched = true;
        self.settings_changed_at = Some(std::time::Instant::now());
        self.grammar.clear();
        self.state.pending.clear();
    }

    /// Whether `key` begins one of the user's bindings: the Keys page's, or `dacode.map`'s in this
    /// mode.
    fn user_claims(&self, key: Key) -> bool {
        use dacode_input::Match;
        dacode_input::shortcuts::current().lookup(&[key]) != Match::None
            || self.grammar.keymap().lookup(self.state.mode, &[key]) != Match::None
    }

    /// Puts a command back on the keys it was built with: the ⟲ on its row, or Delete.
    fn restore_keys(&mut self, command: &'static dacode_input::Command) {
        let modal = self.state.modal;
        if dacode_input::shortcuts::current().differs(command, modal) {
            self.change_shortcuts(|shortcuts| shortcuts.restore(command));
        }
    }

    fn tick_import(&mut self, item: dacode::settings_ui::Importable) {
        if let Some(page) = &mut self.state.settings_page {
            page.tick(item);
        }
    }

    /// Opens `init.lua` as a file — what the Keys page's link does. Whatever the page is still
    /// waiting to write goes in first, so the file opened is the one the page describes; and a
    /// config directory with no `init.lua` gets an empty one, since opening a file that is not there
    /// is refused.
    fn open_init_lua(&mut self) {
        self.flush_settings();
        let path = self.config_dir.join("init.lua");
        if !path.exists() {
            let made = std::fs::create_dir_all(&self.config_dir)
                .and_then(|()| dacode::save::write(&path, b""));
            if let Err(error) = made {
                self.state.hover = Some(format!("could not create {}: {error}", path.display()));
                return;
            }
        }
        self.activate(Target::File(path));
        self.state.hover = Some(
            dacode::settings_ui::Link::RebindKeys
                .explanation()
                .to_string(),
        );
    }

    /// Puts a setting back to its default: the ⟲, or delete on its row.
    ///
    /// The field's line is taken out of the block rather than rewritten at the default, so the file
    /// stays as short as what the user chose, and a later change to DaCode's default — or a value
    /// imported from VSCode — reaches them. With a line of their own above the block setting the
    /// field, that line is what then applies; the page shows the default until the file is next read.
    fn reset_setting(&mut self, field: dacode::settings_ui::Field) {
        if field.resettable() && !field.is_default(&self.state.setting_values()) {
            self.change_setting(field, |values| values.reset(field));
            self.settings_touched.retain(|touched| *touched != field);
            if !self.settings_reset.contains(&field) {
                self.settings_reset.push(field);
            }
        }
    }

    /// Opens the settings tab — the gear, `ctrl-,` and the palette's "settings" — or shows it when
    /// it is already open. The language-server report is gathered here, once, because finding a
    /// program walks `PATH` and must never happen while drawing.
    fn open_settings_tab(&mut self) {
        if self.state.settings_page.is_none() {
            let mut page = dacode::settings_ui::Page::new();
            page.set_reports(
                self.detect_servers(),
                self.detect_formatters(),
                self.state.modal,
            );
            self.state.settings_page = Some(page);
            self.state.snap_settings_scroll();
        }
        self.state.settings_active = true;
        // What would otherwise take its keys belongs to the text it is covering.
        self.state.completion = None;
        self.state.focus = dacode::focus::Focus::Editor;
    }

    /// Puts the file back in front of the settings tab, which stays open beside it. The one way the
    /// tab is hidden, because the theme dropdown hangs from it: a list left open behind a hidden tab
    /// came back with the tab and took the next key — and a dropped file hides the tab with no key
    /// or click reaching the list first.
    fn hide_settings_tab(&mut self) {
        self.state.settings_active = false;
        self.close_dropdown();
        if let Some(page) = &mut self.state.settings_page {
            page.cancel_capture();
        }
    }

    /// Closes the settings tab: its close button, or `ctrl-w` while it is the tab on screen. A change
    /// still waiting to be written is not thrown away with it — the idle step writes it as before.
    fn close_settings_tab(&mut self) {
        self.state.settings_page = None;
        self.state.settings_active = false;
    }

    /// Applies a change immediately. A settings page whose effect you cannot see is a form.
    fn adjust_setting(&mut self, field: dacode::settings_ui::Field, delta: i32) {
        self.change_setting(field, |values| values.adjust(field, delta));
    }

    /// The click path: like `adjust(+1)`, but a number at its limit wraps rather than sticking.
    fn cycle_setting(&mut self, field: dacode::settings_ui::Field) {
        self.change_setting(field, |values| values.cycle(field));
    }

    fn change_setting(
        &mut self,
        field: dacode::settings_ui::Field,
        change: impl FnOnce(&mut dacode::settings_ui::Values),
    ) {
        let mut values = self.state.setting_values();
        change(&mut values);
        self.apply_setting(field, &values);
        self.settings_changed(field, std::time::Instant::now());
    }

    /// Puts one field of `values` into the live editor: what a change on the page does, and what a
    /// reload does again for the changes still waiting to be written.
    fn apply_setting(
        &mut self,
        field: dacode::settings_ui::Field,
        values: &dacode::settings_ui::Values,
    ) {
        use dacode::settings_ui::Field;
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
                self.state.typography = dacode_theme::Typography::with_code_size(values.font_size);
                if let Some(window) = &self.window {
                    let scale = self.scale();
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
            Field::InlineBlame => {
                self.state.blame.inline = values.inline_blame;
                self.blame_wanted = true;
            }
            Field::Autosave => self.state.autosave = values.autosave,
            Field::Transparency => self.set_transparency(values.transparency),
            Field::Animations => self.state.chrome.motion.set_animations(values.animations),
            Field::Updates => self.state.chrome.updates = values.updates,
            Field::InterfaceZoom => {
                self.state.chrome.zoom = values.interface_zoom;
                let (width, scale) = (self.window_width(), self.scale());
                fit_wrap_columns(&mut self.state, width, scale);
            }
        }
    }

    /// A setting has changed: it is written after the pause, and its row glows.
    ///
    /// The glow is kicked here rather than synced, because a change is an event and not a state —
    /// the same as the bell's wobble. The grammar decides which rows the Keys page has, so after
    /// the modal switch the selection is put back on one that exists.
    fn settings_changed(&mut self, field: dacode::settings_ui::Field, now: std::time::Instant) {
        self.settings_changed_at = Some(now);
        self.settings_reset.retain(|reset| *reset != field);
        if !self.settings_touched.contains(&field) {
            self.settings_touched.push(field);
        }
        let glow = dacode::motion::Key::Glow(dacode::settings_ui::glow_index(field));
        let motion = &mut self.state.chrome.motion;
        motion.snap(glow, 1.0);
        motion.to(glow, 0.0, dacode::motion::Spring::GLOW, now);
        let modal = self.state.modal;
        if let Some(page) = &mut self.state.settings_page {
            page.select(page.selected(), modal);
        }
    }

    /// Writes the settings to `init.lua` once the last change has waited [`SETTINGS_SAVE_AFTER`].
    /// Returns whether it wrote — or tried to: a failure is said on the status line, and so in the
    /// bell, and not tried again until something changes.
    ///
    /// Called by the idle step with the clock, and by the tests with chosen instants.
    ///
    /// A write held back by an open, unsaved `init.lua` is due again as soon as that buffer is saved
    /// or closed — both are input, so the idle step that follows them is where it happens.
    fn save_settings_if_due(&mut self, now: std::time::Instant) -> bool {
        let due = match self.settings_changed_at {
            Some(changed) => now >= changed + SETTINGS_SAVE_AFTER,
            None => self.settings_blocked && !self.config_buffer_dirty(),
        };
        if due {
            self.flush_settings();
        }
        due
    }

    /// Whether `init.lua` is open in a buffer with unsaved edits.
    fn config_buffer_dirty(&self) -> bool {
        self.buffers_showing(&self.config_dir.join("init.lua"))
            .into_iter()
            .any(|index| self.buffer_is_dirty(index))
    }

    /// When the settings write will be due, if a change is waiting for it. Like autosave's quiet
    /// period it is a time and not an event — nothing wakes the loop at the end of it — so the loop's
    /// sleep has to end there. `None` once the moment is reached, when the idle step acts on it: a
    /// deadline in the past would spin the loop.
    fn settings_save_due_at(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        let due = self.settings_changed_at? + SETTINGS_SAVE_AFTER;
        (due > now).then_some(due)
    }

    /// Puts what the settings tab just wrote into every buffer showing `init.lua` — all of them
    /// clean, or the write would not have happened — and records the file's new stamp, so the next
    /// save of that buffer neither refuses ("changed on disk") nor writes the old block back.
    fn reload_config_buffers(&mut self, path: &std::path::Path, contents: &str) {
        let showing = self.buffers_showing(path);
        for index in &showing {
            match stored_index(*index, self.state.active_buffer) {
                None => {
                    let cursor = self.state.cursor();
                    let previous = self.state.document.version();
                    // As the watcher reloads a file changed on disk, versions kept increasing.
                    self.state.document = Document::from_str(contents);
                    self.state.document.resume_from(previous);
                    let clamped = cursor.min(self.state.document.text().len_chars());
                    self.state.document.set_primary_caret(clamped);
                    self.after_buffer_change();
                }
                Some(stored) => {
                    if let Some(buffer) = self.buffers.get_mut(stored) {
                        let previous = buffer.document.version();
                        buffer.document = Document::from_str(contents);
                        buffer.document.resume_from(previous);
                    }
                }
            }
        }
        if !showing.is_empty() {
            self.remember_disk(&path.to_string_lossy());
            self.refresh_buffer_tabs();
        }
    }

    /// Writes whatever settings change is waiting, now.
    ///
    /// Not under an open `init.lua` with unsaved edits: writing the file would be overwritten by
    /// that buffer's next save, or would make it refuse to save. The write waits, says so, and is
    /// made once the buffer is saved or closed.
    fn flush_settings(&mut self) {
        let pending =
            self.settings_changed_at.take().is_some() | std::mem::take(&mut self.settings_blocked);
        if !pending {
            return;
        }
        if self.config_buffer_dirty() {
            self.settings_blocked = true;
            self.state.hover = Some(
                "init.lua has unsaved edits; settings not written — they will be once it is saved \
                 or closed"
                    .to_string(),
            );
            return;
        }
        match self.write_settings() {
            Ok(()) => {
                self.settings_touched.clear();
                self.settings_reset.clear();
                self.keys_touched = false;
            }
            Err(message) => {
                eprintln!("dacode: {message}");
                self.state.hover = Some(message);
            }
        }
    }

    /// What leaving the editor writes, by either door — the window's close button or quit: the
    /// recovery copy of unsaved work, and a settings change still waiting out its pause.
    ///
    /// A write held for an open init.lua with unsaved edits is made here regardless: those edits are
    /// left in the recovery file, not saved, so the file on disk is the one the page was waiting to
    /// write into, and quitting without it would lose the change the status line promised to keep.
    fn write_before_leaving(&mut self) {
        self.persist_recovery(true);
        if self.settings_blocked || self.settings_changed_at.is_some() {
            self.settings_changed_at = None;
            self.settings_blocked = false;
            match self.write_settings() {
                Ok(()) => {
                    self.settings_touched.clear();
                    self.settings_reset.clear();
                    self.keys_touched = false;
                }
                Err(message) => eprintln!("dacode: {message}"),
            }
        }
    }

    /// Changes how see-through the window is.
    ///
    /// Between two non-zero values this is immediate. From opaque it cannot be: whether a window can
    /// show the desktop through it is decided when the window and its surface are made, and remaking
    /// them mid-session would drop the GPU state everything is drawn with. So it says so, rather
    /// than leaving a setting that appears to do nothing.
    fn set_transparency(&mut self, percent: u8) {
        self.state.chrome.transparency = f32::from(percent.min(90)) / 100.0;
        let showing = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.is_translucent());
        if percent > 0 && !showing && self.renderer.is_some() {
            self.state.hover = Some(
                "transparency takes effect when DaCode next starts — the window was made opaque"
                    .to_string(),
            );
        }
    }

    /// Asks for a colour-scheme file.
    fn prompt_for_theme_file(&mut self) {
        let mut prompt = dacode::prompt::Prompt::new(PromptKind::LoadTheme, self.state.cursor());
        // Pre-filled with the home directory, since a downloaded theme is usually in Downloads.
        prompt.input = format!("{}/", dacode::paths::home().display());
        self.state.prompt = Some(prompt);
    }

    /// Reads a colour scheme in any supported format, applies it, and keeps it.
    ///
    /// Converted to DaCode's own TOML and stored under `themes/`, so it survives a restart and can be
    /// named in `init.lua` — a theme that only lasted for one session would be a demo, not an import.
    fn load_theme_file(&mut self, raw: &str) {
        let expanded = match raw.strip_prefix("~/") {
            Some(rest) => dacode::paths::home().join(rest),
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

        let mut report = dacode_import::ImportReport::default();
        let Some((theme, recognised)) = dacode_import::read_palette(&name, &text, &mut report)
        else {
            self.warn(&format!(
                "not a colour scheme dacode can read ({}): {}",
                dacode_import::Format::EXTENSIONS,
                report.notes().first().cloned().unwrap_or_default()
            ));
            return;
        };

        self.state.theme = theme;

        // Kept as DaCode's own format, under a slug of the file's name.
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

        let directory = self.config_dir.join("themes");
        let path = directory.join(format!("{slug}.toml"));
        if std::fs::create_dir_all(&directory).is_ok()
            && std::fs::write(&path, dacode::import_cli::render_theme(&slug, &theme)).is_ok()
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
        let Some(page) = &self.state.settings_page else {
            return;
        };
        let checklist = page.checklist();
        if checklist.is_empty() {
            self.state.hover =
                Some("tick what to import first — nothing is imported unless you ask".into());
            return;
        }

        let directory = self.config_dir.clone();
        // The report goes where every other message goes — the status line, and from there the
        // bell — rather than into the list, where it would be one more row to scroll past.
        match dacode::import_cli::import_from_installed_vscode(checklist, &directory) {
            Ok(report) => {
                let rendered = report.render();
                eprintln!("{rendered}");

                // The imported files are only config; reloading is what makes them take effect, and
                // doing it here means the page's own values update too.
                self.reload_config();
                self.state.hover = Some(rendered);
            }
            Err(error) => self.state.hover = Some(format!("import failed: {error}")),
        }
    }

    /// Writes the page's values into the marked block of `init.lua`, through `save::write`.
    ///
    /// The file is read first and everything outside the block kept. A file that exists and cannot be
    /// read is left alone: writing the block alone over it would replace the user's whole config
    /// with twelve lines, and an automatic save must never be the thing that does that.
    fn write_settings(&mut self) -> Result<(), String> {
        let directory = self.config_dir.clone();
        let path = directory.join("init.lua");
        let existing = match std::fs::read_to_string(&path) {
            Ok(existing) => existing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(format!(
                    "settings not saved: could not read {}: {error}",
                    path.display()
                ));
            }
        };
        // Only the blocks something changed: a key moved on its own must not leave an empty
        // `dacode.set {}` in a file that had none, nor a setting rewrite the keys.
        let mut updated = existing.clone();
        if !self.settings_touched.is_empty() || !self.settings_reset.is_empty() {
            updated = dacode::settings_ui::write_touched(
                &updated,
                &self.state.setting_values(),
                &self.settings_touched,
                &self.settings_reset,
            );
        }
        if self.keys_touched {
            updated =
                dacode::settings_ui::write_keys(&updated, &dacode_input::shortcuts::current());
        }

        std::fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "settings not saved: could not create {}: {error}",
                directory.display()
            )
        })?;

        // The editor's own write, so the watcher does not treat it as an outside edit and reload.
        let key = dacode_project::canonical(&path);
        self.self_written.insert(key.clone());
        match dacode::save::write(&path, updated.as_bytes()) {
            Ok(()) => {
                self.self_written.insert(dacode_project::canonical(&path));
                self.page_wrote = Some(updated.clone());
                let live = self.state.setting_values();
                for field in self.settings_touched.iter().chain(&self.settings_reset) {
                    self.file_values.copy(*field, &live);
                }
                self.reload_config_buffers(&path, &updated);
                Ok(())
            }
            Err(error) => {
                self.self_written.remove(&key);
                Err(format!(
                    "settings not saved: could not write {}: {error}",
                    path.display()
                ))
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
                    // Opening a file is going to it: the next key is typed into it, not into the
                    // tree. Expanding a folder is not, so it keeps the keys.
                    self.state.focus = dacode::focus::Focus::Editor;
                }
            }
            _ => return false,
        }
        true
    }

    /// A click in the search view: a box takes the keys, a toggle flips, All replaces, a hit opens.
    fn click_search_panel(&mut self, sidebar: Rect, x: f32, y: f32, scale: f32) {
        use dacode::layout::contains;
        let root = self.state.project_root.clone();
        let Some(panel) = &mut self.state.search else {
            return;
        };
        let geometry = panel.geometry(sidebar, scale);
        self.state.focus = dacode::focus::Focus::Sidebar;
        if let Some((toggle, _)) = geometry
            .toggles
            .iter()
            .find(|(_, cell)| contains(*cell, x, y))
        {
            panel.flip(*toggle, &root);
        } else if contains(geometry.query, x, y) {
            panel.field = dacode::search_panel::Field::Query;
        } else if contains(geometry.replace, x, y) {
            panel.field = dacode::search_panel::Field::Replace;
        } else if contains(geometry.replace_all, x, y) {
            self.press_replace_all();
        } else if let Some(row) = geometry.row_at(x, y)
            && panel.select_row(row)
        {
            self.open_search_hit();
        }
    }

    /// Keys for the search view while it has focus. Returns true if the key was consumed.
    ///
    /// Printable keys type into the focused box, as they would into any search box; the arrows walk
    /// the hits and Enter opens one. Ctrl and alt combinations go on to the bindings, except the
    /// toggles' own — alt-c, alt-w and alt-r, VSCode's — and ctrl-alt-enter, replace all.
    fn handle_search_key(&mut self, key: Key) -> bool {
        use dacode::search_panel::Toggle;
        if !self.state.search_showing() {
            return false;
        }
        let root = self.state.project_root.clone();
        let capacity = self.search_capacity();
        let Some(panel) = &mut self.state.search else {
            return false;
        };
        if key.alt && !key.ctrl {
            let toggle = match key.code {
                KeyCode::Char('c') => Toggle::Case,
                KeyCode::Char('w') => Toggle::Word,
                KeyCode::Char('r') => Toggle::Regex,
                _ => return false,
            };
            panel.flip(toggle, &root);
            return true;
        }
        if key.ctrl && key.alt && key.code == KeyCode::Enter {
            self.press_replace_all();
            return true;
        }
        if key.ctrl || key.alt {
            return false;
        }
        match key.code {
            KeyCode::Char(c) => {
                let mut buffer = [0u8; 4];
                panel.type_text(c.encode_utf8(&mut buffer), &root);
            }
            KeyCode::Backspace => panel.backspace(&root),
            KeyCode::Tab => panel.other_field(),
            KeyCode::Down => panel.move_selection(1, capacity),
            KeyCode::Up => panel.move_selection(-1, capacity),
            KeyCode::Enter => self.open_search_hit(),
            _ => return false,
        }
        true
    }

    /// How many result rows the search view has room for, from the same geometry it draws with.
    fn search_capacity(&self) -> usize {
        let (Some(sidebar), Some(panel)) = (self.layout().sidebar, &self.state.search) else {
            return 1;
        };
        panel.geometry(sidebar, self.scale()).capacity
    }

    /// Opens the selected hit at its line and column, and gives the keys to the text.
    fn open_search_hit(&mut self) {
        let Some((path, line, byte)) = self
            .state
            .search
            .as_ref()
            .and_then(|panel| panel.selection())
            .map(|hit| (hit.path.clone(), hit.line_number, hit.range.start))
        else {
            return;
        };
        let here = self.here();
        if Some(path.to_string_lossy().as_ref()) != self.state.file_path.as_deref() {
            self.activate(Target::File(path.clone()));
        }
        // Only if it opened: a refused file leaves the buffer that was there, and moving its caret
        // to another file's line would be a jump to nowhere.
        if buffer_key(self.state.file_path.as_deref()) != Some(dacode_project::canonical(&path)) {
            return;
        }
        let text = self.state.document.text();
        let target = (line.saturating_sub(1) as usize).min(text.last_line());
        // The hit's range is in bytes of the line, and the rope counts characters.
        let column = text
            .line(target)
            .to_string()
            .get(..byte)
            .map_or(0, |prefix| prefix.chars().count());
        let offset = text.point_to_char(dacode_text::Point::new(target, column));
        self.state.document.set_primary_caret(offset);
        self.jumps.push(here);
        self.state.focus = dacode::focus::Focus::Editor;
        self.reveal_cursor();
    }

    /// Replace all, which asks to be pressed twice: it rewrites files and cannot be undone.
    fn press_replace_all(&mut self) {
        let skip = self.dirty_buffer_keys();
        let root = self.state.project_root.clone();
        let waker = self.waker.clone();
        let Some(panel) = &mut self.state.search else {
            return;
        };
        if panel.query.is_empty() || panel.hits().is_empty() || panel.replacing() {
            return;
        }
        if !panel.confirming {
            panel.confirming = true;
            return;
        }
        panel.start_replace(&root, skip, waker);
    }

    /// The canonical paths of every buffer with unsaved changes.
    fn dirty_buffer_keys(&self) -> Vec<std::path::PathBuf> {
        let live = self
            .state
            .document
            .is_dirty()
            .then(|| buffer_key(self.state.file_path.as_deref()))
            .flatten();
        self.buffers
            .iter()
            .filter(|buffer| buffer.document.is_dirty())
            .filter_map(|buffer| buffer_key(buffer.file_path.as_deref()))
            .chain(live)
            .collect()
    }

    /// Takes the search view's hits and a finished replace-all. Returns whether anything changed.
    fn poll_search_panel(&mut self) -> bool {
        let root = self.state.project_root.clone();
        let Some(panel) = &mut self.state.search else {
            return false;
        };
        let mut changed = panel.poll();
        if let Some(report) = panel.poll_replace() {
            // Searched again, so what is listed is what the files now hold.
            panel.restart(&root);
            self.state.hover = Some(report.describe());
            changed = true;
        }
        changed
    }

    /// Keys for the results panel. Returns true if the key was consumed.
    fn handle_results_key(&mut self, key: Key) -> bool {
        let Some(results) = &mut self.state.results else {
            return false;
        };

        match key.code {
            KeyCode::Escape => {
                self.state.results = None;
                self.state.focus = dacode::focus::Focus::Editor;
                true
            }
            KeyCode::Enter => {
                self.open_hit();
                self.state.focus = dacode::focus::Focus::Editor;
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
        self.write_active(Saving::Asked);
    }

    /// The one path every save of a buffer takes.
    ///
    /// `ctrl-s` used to take another: the executor's own `Save` arm, which wrote the file and did
    /// none of what follows — the recovery copy stayed behind to be offered again, the watcher saw
    /// the write as someone else's, and `format_on_save` never ran for the key most people press.
    fn write_active(&mut self, saving: Saving) {
        // A viewed file's buffer is not its contents — an image's is empty, a document's is the
        // text read out of it — so writing it would replace a PDF with nothing, or a .docx with
        // plain text. It is refused before anything else, autosave included.
        if self.state.preview.is_some() {
            if saving == Saving::Asked {
                self.warn("this tab views the file — DaCode does not write it");
            }
            return;
        }
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
        let file = std::path::Path::new(&path);
        let key = buffer_key(Some(&path));
        let before = dacode::save::Stamp::of(file);

        let recorded = key.as_ref().and_then(|key| self.on_disk.get(key)).copied();
        if dacode::save::changed_since(recorded, before) {
            let confirmed = saving == Saving::Asked && key.is_some() && self.overwrite == key;
            if !confirmed {
                self.refuse_overwrite(&path, key, saving);
                return;
            }
        }
        self.overwrite = None;

        let text = self.state.document.text().to_string();
        let canonical = dacode_project::canonical(file);
        self.self_written.insert(canonical.clone());
        // Saved by rename, which the watcher reports as the file appearing — and a file appearing
        // in the project is what starts a fresh walk. Told first, so a save is not a reindex.
        if before.is_some() {
            if let Some(watcher) = &mut self.watcher {
                watcher.replacing(&canonical);
            }
        }

        match dacode::save::write(file, text.as_bytes()) {
            Ok(()) => {
                // Canonicalization only works once the file exists, so a first save records it here.
                self.self_written.insert(dacode_project::canonical(file));
                self.remember_disk(&path);
                self.state.document.mark_saved();
                // A save is the moment the buffer and the blame's idea of it are re-aligned: the
                // edit log's pessimism about split lines ends here.
                self.blame_wanted = true;
                // And the file's row in the source-control view appears, or changes letter.
                self.refresh_scm(false);
                // The file on disk is authoritative now, so the recovery copy must go — otherwise it
                // would be offered again on the next open.
                self.recovery.discard(file);
                eprintln!("dacode: wrote {path}");

                // Formatting is a round trip, so it cannot come *before* the write without either
                // blocking the UI thread or risking a save that never happens. The file is saved as
                // asked, then reformatted and saved again when the reply arrives.
                //
                // No longer gated on a language server being up: for a plain JavaScript, CSS or
                // Markdown project there is no server and never will be, and `format_on_save`
                // silently doing nothing there was the whole shape of this defect. `route_format`
                // decides which of the two can actually answer.
                if saving != Saving::AfterFormat && self.state.format_on_save && self.can_format() {
                    self.save_after_format = true;
                    self.state.pending_request = Some(Action::Format);
                }
            }
            // Reported, never silent: a save that failed while the user believes it succeeded is
            // the worst failure an editor has.
            Err(error) => self.warn(&format!("could not write {path}: {error}")),
        }
    }

    /// Declines a save that would overwrite someone else's change, and says how to make it anyway.
    ///
    /// An asked-for save is armed so that asking again overwrites: the user has now been told, and
    /// a second refusal would be an editor that cannot save. Autosave is armed too — the person
    /// pressing the key next has read the message — but it never *takes* the arming, so a file is
    /// only ever overwritten by someone who chose to.
    fn refuse_overwrite(&mut self, path: &str, key: Option<std::path::PathBuf>, saving: Saving) {
        self.overwrite = key;
        let name = std::path::Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let again = match (self.state.modal, saving) {
            (true, Saving::Asked) => ":w again overwrites it, :e loads theirs".to_string(),
            (true, _) => ":w overwrites it, :e loads theirs".to_string(),
            (false, Saving::Asked) => format!("{} again overwrites it", self.key_for("save")),
            (false, _) => format!("{} overwrites it", self.key_for("save")),
        };

        match saving {
            Saving::Asked => self.warn(&format!(
                "{name} changed on disk since it was opened — {again}"
            )),
            // On the hover line, not in a prompt: autosave fires the moment typing pauses, and a
            // prompt would take the next keystroke. Pushed back to the next edit, or it would ask
            // the filesystem and say the same thing on every wake.
            Saving::Auto | Saving::AfterFormat => {
                self.last_edit = None;
                self.state.hover = Some(format!(
                    "not saved: {name} changed on disk since it was opened — {again}"
                ));
            }
        }
    }

    /// Records what `path` looks like on disk now, as the version this editor last saw.
    ///
    /// Called wherever a buffer's text and its file agree: opening, reloading, saving. A site that
    /// forgets leaves no record, which turns the overwrite check off for that file rather than
    /// making it refuse to save.
    fn remember_disk(&mut self, path: &str) {
        let Some(key) = buffer_key(Some(path)) else {
            return;
        };
        match dacode::save::Stamp::of(std::path::Path::new(path)) {
            Some(stamp) => self.on_disk.insert(key, stamp),
            None => self.on_disk.remove(&key),
        };
    }

    fn open_palette(&mut self, kind: PaletteKind) {
        let mut palette = Palette::new(kind);
        match kind {
            PaletteKind::Files => palette.refresh_files(&mut self.index),
            PaletteKind::Buffers => palette.refresh_buffers(&self.buffer_names()),
            PaletteKind::Commands => palette.refresh_commands(self.state.modal),
            // Filled when the server answers.
            PaletteKind::CodeActions | PaletteKind::Symbols | PaletteKind::ProjectSymbols => {}
            // What git said last time, until it answers again.
            PaletteKind::Branches => palette.refresh_branches(&self.branches),
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
            KeyCode::Escape => self.state.close_palette(),
            KeyCode::Enter => {
                let chosen = palette.selection().map(|row| row.target.clone());
                self.state.close_palette();
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
                PaletteKind::Branches => palette.refresh_branches(&self.branches),
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

                match dacode::open::read(&path) {
                    dacode::open::Opened::Text(contents) => {
                        self.remember_disk(&path_string);
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
                    // `open_document` reaches `after_buffer_change`, which starts the viewer.
                    dacode::open::Opened::View(_) => self.open_document(
                        Document::from_str(""),
                        relative_crumbs(&path_string),
                        Some(path_string),
                    ),
                    // On screen, not to a console: a release build on Windows has none, so this
                    // was a click on a file in the tree that appeared to do nothing at all.
                    dacode::open::Opened::Refused(why) => {
                        eprintln!("dacode: {why}");
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
            Target::Branch { name, remote } => {
                self.run_git(dacode::scm::Op::Checkout { name, remote })
            }
            Target::CreateBranch(name) if name.is_empty() => {
                let _ = self.run(Action::Git(dacode_input::Git::CreateBranch));
            }
            Target::CreateBranch(name) => self.run_git(dacode::scm::Op::CreateBranch(name)),
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
        let offset = dacode_lsp::from_lsp(self.state.document.text(), symbol.position);
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
                dacode::edits::apply_workspace_edit(
                    document,
                    path.as_deref(),
                    &action.edit,
                    version,
                )
            })
        });
        self.code_actions.clear();
    }

    /// Gives the active buffer its viewer, when its file is one the viewer shows and it has none
    /// yet — at startup, after a session restore, and on every switch, so a buffer restored as an
    /// empty placeholder starts reading its file only once it is looked at. `forced` is
    /// `preview_file`, which views a kind that otherwise opens as text: an SVG.
    ///
    /// An office document's text is read here, on the UI thread, bounded as `open::read` is; its
    /// pictures are not, which is what would make it slow.
    fn ensure_preview(&mut self, forced: bool) {
        if self.state.preview.is_some() {
            return;
        }
        let Some(path) = self.state.file_path.clone() else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let kind = if forced {
            dacode::viewer::Kind::of(&path)
        } else {
            dacode::viewer::Kind::opened(&path)
        };
        let Some(kind) = kind else {
            return;
        };
        if kind.is_drawn() {
            self.state.preview = Some(dacode::viewer::Preview::open(
                kind,
                path,
                self.waker.clone(),
            ));
            return;
        }
        match dacode::viewer::office_text(&path) {
            Ok(text) => self.state.document = Document::from_str(&text),
            Err(why) => self.state.hover = Some(why),
        }
        self.state.preview = Some(dacode::viewer::Preview::open(kind, path, None));
    }

    /// Makes a new document the active one, keeping the previous buffer.
    fn open_document(&mut self, document: Document, path: Vec<String>, file_path: Option<String>) {
        self.hide_settings_tab();
        let previous = Buffer {
            document: std::mem::replace(&mut self.state.document, document),
            path: std::mem::replace(&mut self.state.path, path),
            file_path: std::mem::replace(&mut self.state.file_path, file_path),
            scroll: std::mem::take(&mut self.state.scroll),
            highlights: std::mem::take(&mut self.state.highlights),
            review: self.state.review.take(),
            preview: self.state.preview.take(),
        };
        // An untouched scratch buffer is a placeholder for "nothing open yet", not a document: kept,
        // it sat in the tab strip as `untitled` beside every file opened after it.
        if is_placeholder(&previous) {
            self.panes.buffer_closed(self.state.active_buffer);
        } else {
            self.buffers.insert(self.state.active_buffer, previous);
        }
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
    /// buffer instead, which is what starting DaCode with no argument already gives you.
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
            self.state.review = None;
            self.state.preview = None;
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
    /// Two checks, because neither alone is enough. `dacode_project::within` folds the path lexically,
    /// which is the only thing that works for a file that does not exist yet — and creating one is
    /// the common case here. `canonical` then catches what folding cannot see: a symlink inside the
    /// project pointing out of it. It is asked only where the parent is really on disk, since
    /// otherwise it hands back its input unchanged and answers nothing.
    fn in_project(&self, relative: &str) -> Option<std::path::PathBuf> {
        let path = dacode_project::within(&self.state.project_root, relative, cfg!(windows))?;

        if let Some(parent) = path.parent() {
            if parent.exists()
                && !dacode_project::canonical(parent)
                    .starts_with(dacode_project::canonical(&self.state.project_root))
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
            self.warn("DaCode does not delete folders");
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
        self.walk = Some(dacode_project::FileWalk::start(root));
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
        // Choosing a file is choosing to see it, the live one included: its tab, clicked while the
        // settings tab is showing, brings it back.
        self.hide_settings_tab();
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
            review: std::mem::replace(&mut self.state.review, target.review),
            preview: std::mem::replace(&mut self.state.preview, target.preview),
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
                self.git.request(dacode::git::DiffRequest {
                    path: std::path::PathBuf::from(path),
                    text: self.state.document.text().to_string(),
                    version,
                    revert: None,
                });
                self.diffed_version = Some(version);
            }
        }

        match self.git.poll() {
            Some(dacode::git::GitResult::Diff { path, diff })
                if diff.version() == Some(version) && self.is_active_file(&path) =>
            {
                self.state.diff = diff;
                true
            }
            // Stale: the buffer moved on, or the file did, while the diff ran. Dropped, and the
            // newer request that caused the move is already queued.
            Some(dacode::git::GitResult::Diff { .. }) => false,
            Some(dacode::git::GitResult::Refused(why)) => {
                self.state.hover = Some(why);
                true
            }
            // Counted, not read: it exists so one request always produces one result.
            Some(dacode::git::GitResult::Superseded) => false,
            Some(dacode::git::GitResult::Revert {
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

    /// Keeps the blame in step with the buffer: asks for one when something called for it, and takes
    /// the answer if it is the one being waited for.
    ///
    /// Nothing here is per keystroke. The blame held is moved through edits by its own log (see
    /// [`dacode::blame`]), so the only reasons to ask git again are the ones that set
    /// `blame_wanted`, and the log having given up.
    fn sync_blame(&mut self) -> bool {
        let blame = &self.state.blame;
        if blame.wanted() && (self.blame_wanted || blame.needs_refresh()) {
            if let Some(path) = &self.state.file_path {
                let path = std::path::PathBuf::from(path);
                let version = self.state.document.version();
                self.blame_worker.request(dacode::blame::Request::Blame {
                    path: path.clone(),
                    text: self.state.document.text().to_string(),
                    version,
                });
                self.state.blame.requested(path, version);
            }
            self.blame_wanted = false;
        }

        let mut changed = false;
        while let Some(result) = self.blame_worker.poll() {
            match result {
                dacode::blame::BlameResult::Blame {
                    path,
                    version,
                    blame,
                } => {
                    // `accept` refuses an answer for another file or an older request itself; the
                    // path check here is for the buffer having changed underneath both.
                    if self.is_active_file(&path) {
                        changed |= self.state.blame.accept(&path, version, blame);
                    }
                }
                dacode::blame::BlameResult::Commit { path, text } => {
                    if self.is_active_file(&path) {
                        self.state.hover = Some(text);
                        changed = true;
                    }
                }
                dacode::blame::BlameResult::Superseded => {}
            }
        }

        // The column takes its width from the code, so soft wrap has to be refitted when it comes
        // or goes — a toggle, or git answering that this file has no history.
        let column = self.state.blame_column(0, 1.0).is_some();
        if column != self.blame_fitted {
            self.blame_fitted = column;
            if self.window.is_some() {
                let (width, scale) = (self.window_width(), self.scale());
                fit_wrap_columns(&mut self.state, width, scale);
            }
            changed = true;
        }
        changed
    }

    // ---- source control -------------------------------------------------------------------------

    /// Asks git for a fresh status of the project. `settle` waits for the repository to stop
    /// changing first, for a change seen on disk — which arrives as a burst of events.
    fn refresh_scm(&mut self, settle: bool) {
        let version = self.state.scm.ask();
        self.scm.request(dacode::scm::Request::Status {
            root: self.state.project_root.clone(),
            version,
            settle,
        });
    }

    /// `HEAD` moved — a commit, a checkout, a pull, here or anywhere else. Who wrote each line and
    /// what the gutter diffs against may both have changed under a buffer whose text did not, so
    /// both are asked again. Before this a commit made in a terminal showed in the blame only once
    /// the file was saved or reopened.
    fn history_moved(&mut self) {
        self.blame_wanted = true;
        self.diffed_version = None;
    }

    /// Takes what the source-control worker has answered, and notices the repository changing on
    /// disk. Returns whether anything on screen changed.
    fn sync_scm(&mut self) -> bool {
        use dacode::scm::Answer;
        let mut changed = false;
        if let Some(change) = self
            .watcher
            .as_mut()
            .and_then(|watcher| watcher.repository_changed())
        {
            if change.history {
                self.history_moved();
            }
            self.refresh_scm(true);
        }
        while let Some(answer) = self.scm.poll() {
            match answer {
                Answer::Status { version, repo } => {
                    if self.state.scm.accept(version, repo) {
                        changed = true;
                        self.watch_repository();
                    }
                }
                Answer::Done { op, result } => {
                    self.finish_git(op, result);
                    changed = true;
                }
                Answer::Branches(result) => {
                    match result {
                        Ok(branches) => self.branches = branches,
                        Err(failure) => self.state.hover = Some(failure.sentence),
                    }
                    if self
                        .state
                        .palette
                        .as_ref()
                        .is_some_and(|palette| palette.kind == PaletteKind::Branches)
                    {
                        self.refresh_palette();
                    }
                    changed = true;
                }
                Answer::Shown { show, text } => {
                    self.show_review(show, text);
                    changed = true;
                }
                Answer::Superseded => {}
            }
        }
        changed
    }

    /// Watches the repository the last status found, so a commit or a checkout made outside the
    /// editor is noticed when it happens rather than at the next save.
    fn watch_repository(&mut self) {
        let Some(status) = self.state.scm.status() else {
            return;
        };
        if status.git_dir.as_os_str().is_empty() {
            return;
        }
        let (git_dir, common) = (status.git_dir.clone(), status.common_dir.clone());
        if let Some(watcher) = &mut self.watcher {
            if !watcher.watch_repository(&git_dir, &common) {
                eprintln!(
                    "dacode: could not watch {}; changes made outside the editor show on refresh",
                    git_dir.display()
                );
            }
        }
    }

    /// An operation finished: say what it did, and ask what the repository looks like now.
    fn finish_git(&mut self, op: dacode::scm::Op, result: Result<String, dacode::scm::Failure>) {
        use dacode::scm::Op;
        self.state.scm.busy = None;
        match result {
            Ok(said) => {
                if let Op::Commit { message, .. } = &op {
                    // Only if it is still what was committed: typed on while git ran, it is a new one.
                    if self.state.scm.message.trim_end() == message {
                        self.state.scm.take_message();
                    }
                    self.state.scm.amend = false;
                }
                self.state.hover = Some(said);
            }
            Err(failure) if failure.sign_in => {
                self.state.hover = Some(format!(
                    "{} needs you to sign in, and nothing here can ask — run it once in the \
                     terminal ({}), then try again",
                    op.command(),
                    self.key_for("toggle_terminal")
                ));
            }
            Err(failure) => self.state.hover = Some(failure.sentence),
        }
        // A stage leaves `HEAD` where it was; everything else may have moved it, and a failed pull
        // may still have fetched.
        if !matches!(
            op,
            Op::Stage(_) | Op::Unstage(_) | Op::StageAll | Op::UnstageAll
        ) {
            self.history_moved();
        }
        self.refresh_scm(false);
    }

    /// Hands an operation to the worker, in the repository the last status found.
    fn run_git(&mut self, op: dacode::scm::Op) {
        let root = match (&op, self.state.scm.status()) {
            (dacode::scm::Op::Init, _) => self.state.project_root.clone(),
            (_, Some(status)) => status.root.clone(),
            (_, None) => {
                self.state.hover = Some(self.no_repository());
                return;
            }
        };
        self.state.scm.busy = Some(op.doing());
        self.scm.request(dacode::scm::Request::Run { root, op });
    }

    /// Why a command that needs a repository did nothing.
    fn no_repository(&self) -> String {
        use dacode::source_control::State;
        match &self.state.scm.state {
            State::NotARepository => {
                "this folder is not a git repository — the source control view can initialize one"
                    .to_string()
            }
            State::NoGit => "git is not installed, or not on PATH".to_string(),
            State::Failed(why) => why.clone(),
            State::Unknown | State::Ready(_) => "still asking git about this folder".to_string(),
        }
    }

    /// The row the source-control view has selected, while it has the keys.
    fn scm_row(&self) -> Option<dacode::source_control::Row> {
        let view = &self.state.scm;
        (self.state.focus == dacode::focus::Focus::Sidebar
            && self.state.source_control_showing()
            && view.field == dacode::source_control::Field::List)
            .then(|| view.selected.and_then(|at| view.rows().get(at).copied()))
            .flatten()
    }

    /// The change a per-file command is about: the selected file while the view has the keys, the
    /// tree's selection or the file on screen otherwise — with the group it was chosen from, when
    /// it was chosen from one. `None` with a sentence said when there is nothing to act on.
    fn git_target(&mut self) -> Option<(Option<dacode::scm::Group>, dacode::scm::Entry)> {
        if let Some(dacode::source_control::Row::File { group, entry }) = self.scm_row() {
            let entry = self.state.scm.status()?.entries.get(entry)?.clone();
            return Some((Some(group), entry));
        }
        let relative = self.tree_target();
        let found = relative.as_ref().and_then(|relative| {
            let status = self.state.scm.status()?;
            let inside = status.relative(&self.state.project_root.join(relative))?;
            status.find(&inside).cloned()
        });
        if found.is_none() {
            self.state.hover = Some(match relative {
                Some(relative) => format!("{relative} has no changes"),
                None => "no file to act on".to_string(),
            });
        }
        found.map(|entry| (None, entry))
    }

    /// Does what a source-control command asks.
    fn git_action(&mut self, git: dacode_input::Git) {
        use dacode::scm::{Group, Op};
        use dacode::source_control::{Confirm, Row};
        use dacode_input::Git;
        let needs_repository = !matches!(git, Git::Init | Git::Refresh | Git::Menu);
        if needs_repository && self.state.scm.status().is_none() {
            self.state.hover = Some(self.no_repository());
            return;
        }
        match git {
            Git::Commit => self.commit(false),
            Git::CommitAmend => {
                self.state.scm.amend = true;
                self.commit(true)
            }
            Git::Refresh => {
                self.history_moved();
                self.refresh_scm(false);
            }
            Git::Stage | Git::Unstage => {
                let stage = git == Git::Stage;
                if let Some(Row::Header(group)) = self.scm_row() {
                    let op = match (stage, group) {
                        (true, Group::Changes) => Op::StageAll,
                        (true, group) => Op::Stage(self.state.scm.paths(group)),
                        (false, _) => Op::UnstageAll,
                    };
                    self.run_git(op);
                    return;
                }
                let Some((group, entry)) = self.git_target() else {
                    return;
                };
                if stage {
                    let can = group.is_none_or(|group| group != Group::Staged)
                        && (entry.in_group(Group::Changes) || entry.conflict);
                    if !can {
                        self.state.hover = Some(format!("{} has nothing to stage", entry.name()));
                        return;
                    }
                    self.run_git(Op::Stage(vec![entry.path.clone()]));
                } else {
                    if !entry.in_group(Group::Staged) {
                        self.state.hover = Some(format!("{} has nothing staged", entry.name()));
                        return;
                    }
                    let mut paths = vec![entry.path.clone()];
                    paths.extend(entry.original.clone());
                    self.run_git(Op::Unstage(paths));
                }
            }
            Git::Discard => {
                if let Some(Row::Header(Group::Changes)) = self.scm_row() {
                    self.git_action(Git::DiscardAll);
                    return;
                }
                let Some((_, entry)) = self.git_target() else {
                    return;
                };
                if !entry.in_group(Group::Changes) {
                    self.state.hover = Some(format!(
                        "{} has no unstaged changes to discard",
                        entry.name()
                    ));
                    return;
                }
                let confirm = Confirm::Discard(vec![entry.path.clone()]);
                if self.state.scm.confirm.as_ref() != Some(&confirm) {
                    self.state.scm.confirm = Some(confirm);
                    self.state.hover = Some(format!(
                        "discard your changes to {}? Do it again to — it cannot be undone",
                        entry.name()
                    ));
                    return;
                }
                self.state.scm.confirm = None;
                let (tracked, untracked) = if entry.untracked {
                    (Vec::new(), vec![entry.path.clone()])
                } else {
                    (vec![entry.path.clone()], Vec::new())
                };
                self.run_git(Op::Discard { tracked, untracked });
            }
            Git::StageAll => self.run_git(Op::StageAll),
            Git::UnstageAll => self.run_git(Op::UnstageAll),
            Git::DiscardAll => {
                if self.state.scm.count(Group::Changes) == 0 {
                    self.state.hover = Some("nothing to discard".to_string());
                    return;
                }
                if self.state.scm.confirm != Some(Confirm::DiscardAll) {
                    self.state.scm.confirm = Some(Confirm::DiscardAll);
                    self.state.hover = Some(
                        "discard every unstaged change, and delete every untracked file? Do it \
                         again to — it cannot be undone"
                            .to_string(),
                    );
                    return;
                }
                self.state.scm.confirm = None;
                self.run_git(Op::DiscardAll);
            }
            Git::Fetch => self.run_git(Op::Fetch),
            Git::Pull => self.run_git(Op::Pull),
            Git::Push => self.run_git(Op::Push),
            Git::Sync => self.run_git(Op::Sync),
            Git::Stash => self.run_git(Op::Stash),
            Git::PopStash => self.run_git(Op::PopStash),
            Git::Checkout => {
                if let Some(status) = self.state.scm.status() {
                    let root = status.root.clone();
                    self.scm.request(dacode::scm::Request::Branches { root });
                }
                self.state.close_menu();
                self.open_palette(PaletteKind::Branches);
            }
            Git::CreateBranch => {
                self.state.prompt = Some(dacode::prompt::Prompt::new(
                    PromptKind::NewBranch,
                    self.state.cursor(),
                ));
            }
            Git::History => {
                if let Some(status) = self.state.scm.status() {
                    let root = status.root.clone();
                    self.scm.request(dacode::scm::Request::Show {
                        root,
                        show: dacode::scm::Show::History,
                    });
                }
            }
            Git::OpenChanges => self.open_changes(),
            Git::OpenFile => {
                let root = self.state.scm.status().map(|status| status.root.clone());
                if let (Some(root), Some((_, entry))) = (root, self.git_target()) {
                    let path = root.join(&entry.path);
                    if path.is_file() {
                        self.activate(Target::File(path));
                    } else {
                        self.state.hover =
                            Some(format!("{} is not on disk — it was deleted", entry.name()));
                    }
                }
            }
            Git::Init => self.run_git(Op::Init),
            Git::Menu => {
                let at = self
                    .layout()
                    .sidebar
                    .filter(|_| self.state.source_control_showing())
                    .and_then(|sidebar| {
                        let geometry =
                            self.state
                                .scm
                                .geometry(sidebar, self.scale(), &self.state.typography);
                        geometry
                            .tools
                            .iter()
                            .find(|(tool, _)| *tool == dacode::source_control::Tool::More)
                            .map(|(_, rect)| (rect.x, rect.bottom()))
                    })
                    .unwrap_or(self.pointer);
                self.state
                    .open_menu(dacode::menu::Menu::source_control(at, self.state.modal));
            }
        }
    }

    /// Commits what is staged — or, with nothing staged, everything, once asked twice: VSCode's
    /// smart commit, which offers to stage all your changes rather than refusing.
    fn commit(&mut self, amend: bool) {
        use dacode::scm::{Group, Op};
        use dacode::source_control::Confirm;
        if self
            .state
            .scm
            .status()
            .is_some_and(|status| status.has_conflicts())
        {
            self.state.hover = Some(
                "resolve the merge conflicts first — stage each file once it is fixed".to_string(),
            );
            return;
        }
        let message = self.state.scm.message.trim_end().to_string();
        if message.trim().is_empty() && !amend {
            self.state.open_source_control();
            self.state.hover = Some("type a commit message first".to_string());
            return;
        }
        let staged = self.state.scm.count(Group::Staged);
        let changes = self.state.scm.count(Group::Changes);
        let stage_all = staged == 0 && !amend;
        if stage_all {
            if changes == 0 {
                self.state.hover = Some("nothing to commit — the working tree is clean".into());
                return;
            }
            if self.state.scm.confirm != Some(Confirm::StageAllAndCommit) {
                self.state.scm.confirm = Some(Confirm::StageAllAndCommit);
                self.state.hover = Some(format!(
                    "nothing is staged — commit again to stage all {changes} change{} and commit \
                     them",
                    if changes == 1 { "" } else { "s" }
                ));
                return;
            }
        }
        self.state.scm.confirm = None;
        self.run_git(Op::Commit {
            message,
            amend,
            stage_all,
        });
    }

    /// Asks for the diff of the change in question, for a read-only tab.
    fn open_changes(&mut self) {
        use dacode::scm::Side;
        let Some((group, entry)) = self.git_target() else {
            return;
        };
        let side = match group {
            Some(group) => entry.side(group),
            None if entry.untracked => Side::Untracked,
            None if entry.worktree != '.' || entry.conflict => Side::Unstaged,
            None => Side::Staged,
        };
        let Some(root) = self.state.scm.status().map(|status| status.root.clone()) else {
            return;
        };
        self.scm.request(dacode::scm::Request::Show {
            root,
            show: dacode::scm::Show::Changes {
                path: entry.path,
                side,
            },
        });
    }

    /// Shows a diff or the history in a read-only tab — the one already showing it, if there is
    /// one, so clicking a row twice does not open it twice.
    fn show_review(&mut self, show: dacode::scm::Show, text: Result<String, dacode::scm::Failure>) {
        let text = match text {
            Ok(text) => text,
            Err(failure) => {
                self.state.hover = Some(failure.sentence);
                return;
            }
        };
        if text.trim().is_empty() {
            self.state.hover = Some(
                "git shows no difference in the text — the change may be to the file's mode only"
                    .to_string(),
            );
            return;
        }
        if self.state.review.as_ref() != Some(&show) {
            let stored = self
                .buffers
                .iter()
                .position(|buffer| buffer.review.as_ref() == Some(&show));
            match stored {
                Some(stored) => {
                    let index = if stored < self.state.active_buffer {
                        stored
                    } else {
                        stored + 1
                    };
                    self.switch_to(index);
                }
                None => {
                    self.open_document(Document::from_str(&text), review_crumbs(&show), None);
                    self.state.review = Some(show);
                    self.state.diff = self.review_marks();
                    self.refresh_buffer_tabs();
                    return;
                }
            }
        }
        // Already open: its text is replaced, keeping its place, as a reload keeps a file's.
        let point = self.state.cursor_point();
        let previous = self.state.document.version();
        self.state.document = Document::from_str(&text);
        self.state.document.resume_from(previous);
        self.restore_cursor(point.line, point.column);
        self.state.diff = self.review_marks();
    }

    /// A diff tab's changed lines as gutter marks, so `]c` and `[c` walk its hunks. Read once, when
    /// the tab is opened or shown — never per frame — and empty for a file, whose marks are git's.
    fn review_marks(&self) -> dacode::git::Diff {
        if !matches!(self.state.review, Some(dacode::scm::Show::Changes { .. })) {
            return dacode::git::Diff::default();
        }
        let text = self.state.document.text();
        let marks = (0..text.len_lines())
            .filter(|line| matches!(text.line(*line).chars().next(), Some('+' | '-')))
            .filter(|line| {
                let start: String = text.line(*line).chars().take(4).collect();
                !start.starts_with("+++ ") && !start.starts_with("--- ")
            })
            .map(|line| (line, dacode::git::Change::Modified))
            .collect();
        dacode::git::Diff::from_marks(marks, self.state.document.version())
    }

    /// How many rows the source-control list has room for, from the geometry it draws with.
    fn scm_capacity(&self) -> usize {
        let Some(sidebar) = self.layout().sidebar else {
            return 1;
        };
        self.state
            .scm
            .geometry(sidebar, self.scale(), &self.state.typography)
            .capacity
    }

    /// Keys for the source-control view while it has focus. Returns whether the key was used.
    fn handle_scm_key(&mut self, key: Key) -> bool {
        use dacode::source_control::{Field, Row};
        use dacode_input::Git;
        if !self.state.source_control_showing() {
            return false;
        }
        // Commits from anywhere in the view, as in VSCode's box.
        if key.ctrl && !key.alt && key.code == KeyCode::Enter {
            let _ = self.run(Action::Git(Git::Commit));
            return true;
        }
        let capacity = self.scm_capacity();
        match self.state.scm.field {
            Field::Message => {
                if key.ctrl && !key.alt {
                    match key.code {
                        // Pasted here rather than into the file behind the view.
                        KeyCode::Char('v') => {
                            if let Some(text) = dacode::clipboard::get() {
                                self.state.scm.type_text(&text.replace("\r\n", "\n"));
                            }
                            return true;
                        }
                        KeyCode::Backspace => {
                            self.state.scm.message.clear();
                            self.state.scm.caret = 0;
                            return true;
                        }
                        _ => return false,
                    }
                }
                if key.alt {
                    return false;
                }
                let view = &mut self.state.scm;
                match key.code {
                    KeyCode::Char(c) => {
                        let mut buffer = [0u8; 4];
                        view.type_text(c.encode_utf8(&mut buffer));
                    }
                    KeyCode::Enter => view.type_text("\n"),
                    KeyCode::Backspace => view.backspace(),
                    KeyCode::Delete => view.delete(),
                    KeyCode::Left => view.move_caret(-1),
                    KeyCode::Right => view.move_caret(1),
                    KeyCode::Home => view.caret_to_line_edge(false),
                    KeyCode::End => view.caret_to_line_edge(true),
                    KeyCode::Tab => view.field = Field::List,
                    // Off the bottom of the box, into the list — the way the arrows read.
                    KeyCode::Down => {
                        let (line, _) = view.caret_line_column();
                        if line + 1 >= view.message.split('\n').count() {
                            view.field = Field::List;
                            if view.selected.is_none() {
                                view.move_selection(1, capacity);
                            }
                        }
                    }
                    _ => return false,
                }
                true
            }
            Field::List => {
                if key.ctrl || key.alt {
                    return false;
                }
                let selected = self
                    .state
                    .scm
                    .selected
                    .and_then(|at| self.state.scm.rows().get(at).copied());
                match key.code {
                    KeyCode::Down => self.state.scm.move_selection(1, capacity),
                    KeyCode::Up if self.state.scm.selected.is_none_or(|at| at == 0) => {
                        self.state.scm.field = Field::Message;
                    }
                    KeyCode::Up => self.state.scm.move_selection(-1, capacity),
                    KeyCode::PageDown => self.state.scm.move_selection(capacity as isize, capacity),
                    KeyCode::PageUp => self
                        .state
                        .scm
                        .move_selection(-(capacity as isize), capacity),
                    KeyCode::Tab => self.state.scm.field = Field::Message,
                    KeyCode::Enter => match selected {
                        Some(Row::Header(group)) => self.state.scm.toggle(group),
                        Some(Row::File { .. }) => {
                            let _ = self.run(Action::Git(Git::OpenChanges));
                        }
                        None => {}
                    },
                    KeyCode::Left | KeyCode::Right => {
                        if let Some(Row::Header(group)) = selected {
                            let folded = self.state.scm.collapsed[group.index()];
                            if folded == (key.code == KeyCode::Right) {
                                self.state.scm.toggle(group);
                            }
                        }
                    }
                    KeyCode::Char(' ') | KeyCode::Char('s') => {
                        let _ = self.run(Action::Git(Git::Stage));
                    }
                    KeyCode::Char('u') => {
                        let _ = self.run(Action::Git(Git::Unstage));
                    }
                    KeyCode::Char('o') => {
                        let _ = self.run(Action::Git(Git::OpenFile));
                    }
                    _ => return false,
                }
                true
            }
        }
    }

    /// A press in the source-control view: the same geometry it is drawn from decides what it hit.
    fn click_source_control(&mut self, sidebar: Rect, x: f32, y: f32, scale: f32) {
        use dacode::source_control::{Field, Hit, Row, RowAction, Tool};
        use dacode_input::Git;
        let geometry = self
            .state
            .scm
            .geometry(sidebar, scale, &self.state.typography);
        let Some(hit) = self.state.scm.hit(&geometry, x, y, scale) else {
            return;
        };
        let git = match hit {
            Hit::Tool(Tool::Commit) | Hit::Commit => Some(Git::Commit),
            Hit::Tool(Tool::Refresh) => Some(Git::Refresh),
            Hit::Tool(Tool::More) => Some(Git::Menu),
            Hit::Branch => Some(Git::Checkout),
            Hit::Pull => Some(Git::Pull),
            Hit::Push => Some(Git::Push),
            Hit::Init => Some(Git::Init),
            Hit::Message => {
                // The caret goes where the click was, as in any text box.
                let view = &mut self.state.scm;
                view.field = Field::Message;
                let advance = dacode::layout::measured_advance(
                    &self.state.typography,
                    dacode_theme::Role::Ui,
                    scale,
                );
                let line = dacode::source_control::MESSAGE_LINE * scale;
                let row = ((y - geometry.message.y - 5.0 * scale) / line).max(0.0) as usize
                    + view.message_top();
                let column = ((x - geometry.message.x - 8.0 * scale) / advance)
                    .round()
                    .max(0.0) as usize;
                let lines: Vec<&str> = view.message.split('\n').collect();
                let row = row.min(lines.len().saturating_sub(1));
                let before: usize = lines[..row]
                    .iter()
                    .map(|text| text.chars().count() + 1)
                    .sum();
                view.caret = before + column.min(lines[row].chars().count());
                None
            }
            Hit::Row(index) => {
                let view = &mut self.state.scm;
                view.selected = Some(index);
                view.field = Field::List;
                match view.rows().get(index).copied() {
                    Some(Row::Header(group)) => {
                        view.toggle(group);
                        None
                    }
                    // A click opens the diff, as in VSCode, and the keys stay in the list.
                    Some(Row::File { .. }) => Some(Git::OpenChanges),
                    None => None,
                }
            }
            Hit::Action(index, action) => {
                let view = &mut self.state.scm;
                view.selected = Some(index);
                view.field = Field::List;
                Some(match action {
                    RowAction::Open => Git::OpenFile,
                    RowAction::Stage => Git::Stage,
                    RowAction::Unstage => Git::Unstage,
                    RowAction::Discard => Git::Discard,
                    RowAction::StageAll => Git::Stage,
                    RowAction::UnstageAll => Git::Unstage,
                    RowAction::DiscardAll => Git::DiscardAll,
                })
            }
        };
        if let Some(git) = git {
            let _ = self.run(Action::Git(git));
        }
    }

    /// Shows the whole commit that last changed `line`, in the info card.
    ///
    /// The summary is already held; the body is not, because porcelain blame gives only the first
    /// line of each message, so the rest is one `git show` on the worker.
    fn show_commit(&mut self, line: usize) {
        let Some(path) = self.state.file_path.clone() else {
            self.state.hover = Some("a scratch buffer has no history".into());
            return;
        };
        match self.state.blame.at(line) {
            Some(dacode::blame::Attribution::Commit(commit)) => {
                let hash = commit.hash.clone();
                self.blame_worker.request(dacode::blame::Request::Show {
                    path: std::path::PathBuf::from(path),
                    hash,
                });
            }
            Some(dacode::blame::Attribution::Uncommitted) => {
                self.state.hover = Some("this line is not committed yet".into());
            }
            None if self.state.blame.is_pending() => {
                self.state.hover = Some("still asking git who changed this line".into());
            }
            None => {
                // Ask for one, so a second try finds it — unless git has nothing to say, which is
                // the same sentence either way.
                self.blame_wanted = true;
                self.state.hover = Some("no history for this line — is the file in git?".into());
            }
        }
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
        self.git.request(dacode::git::DiffRequest {
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
        computed: dacode_text::Version,
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
        let start = document.point_to_char(dacode_text::Point::new(range.start.min(last), 0));
        let end = if range.end > last {
            document.len_chars()
        } else {
            document.point_to_char(dacode_text::Point::new(range.end, 0))
        };

        let edit = dacode_text::Edit::replace(start..end, text);
        match self.state.mutate(|document| {
            document.apply(
                dacode_text::Transaction::new(vec![edit]),
                dacode_buffer::EditKind::Discrete,
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
            let offset = text.point_to_char(dacode_text::Point::new(target, 0));
            self.state.document.set_primary_caret(offset);
            self.reveal_cursor();
        }
    }

    /// The chrome's geometry for the current window, as the last frame drew it.
    ///
    /// The sidebar is read at that frame's instant rather than the clock's: while it slides, the
    /// mouse has to agree with what is on screen, and the clock has moved on since it was drawn.
    fn layout(&self) -> dacode::layout::Layout {
        let (width, height) = self.window_size();
        dacode::layout::Layout::of(
            Rect::new(0.0, 0.0, width as f32, height as f32),
            self.scale(),
            self.state
                .results
                .as_ref()
                .map(|results| results.wanted_rows()),
            self.state
                .sidebar_fraction_at(self.last_frame.unwrap_or_else(std::time::Instant::now)),
        )
        .with_terminal(
            self.state.terminal.open,
            self.state.terminal.fraction,
            self.state.terminal_folded(),
            self.scale(),
        )
        .with_blame(self.state.blame_column(self.panes.active(), self.scale()))
    }

    /// The terminal panel's rectangles, while it is shown. The same function drawing reads, so the
    /// grid the shell is told it has is the grid on screen.
    fn terminal_rects(&self) -> Option<dacode::layout::TerminalRects> {
        let panel = self.layout().terminal?;
        Some(dacode::layout::terminal_panel(
            panel,
            &self.state.typography,
            self.scale(),
        ))
    }

    /// The shown tab's shell, if there is one.
    fn active_terminal(&self) -> Option<&dacode_terminal::Terminal> {
        self.terminals.get(self.state.terminal.active)
    }

    /// Starts another shell in a tab of its own and shows it. Returns whether it started; a shell
    /// that will not start says why on the status line and leaves the other tabs as they were.
    fn spawn_terminal(&mut self) -> bool {
        let (size, cell) = self
            .terminal_rects()
            .filter(|_| !self.state.terminal_folded())
            .map(|rects| {
                (
                    dacode_terminal::GridSize::new(rects.columns, rects.rows),
                    (rects.cell_width as u16, rects.cell_height as u16),
                )
            })
            .unwrap_or((dacode_terminal::GridSize::new(80, 24), (8, 16)));
        let shells = self.terminal_shells.clone().unwrap_or_else(|| {
            dacode_terminal::shells_for(dacode_terminal::Platform::current(), |name| {
                std::env::var(name).ok()
            })
        });
        match dacode_terminal::Terminal::spawn(dacode_terminal::Spawn {
            shells,
            // The project root, so `cargo build` and `npm test` work as typed — never wherever the
            // desktop happened to launch the editor from.
            directory: self.state.project_root.clone(),
            size,
            cell,
            // Every shell shares the loop's waker; each has its own dirty flag, so each wakes the
            // loop once per batch of output however many there are.
            waker: self.waker.clone(),
        }) {
            Ok(terminal) => {
                terminal.set_palette(dacode::terminal::palette(&self.state.theme));
                self.state.terminal.screen = Some(terminal.take_screen());
                self.terminals.push(terminal);
                self.state.terminal.active = self.terminals.len() - 1;
                self.terminal_gesture = None;
                self.sync_terminal_tabs();
                true
            }
            Err(error) => {
                self.state.hover = Some(format!("the terminal could not start a shell: {error}"));
                false
            }
        }
    }

    /// Shows the terminal, starting a shell if there is none, and gives it the keys if `focus`.
    /// Returns whether a shell is running.
    fn show_terminal(&mut self, focus: bool) -> bool {
        self.state.terminal.open = true;
        if focus {
            self.state.focus = dacode::focus::Focus::Terminal;
        }
        if !self.terminals.is_empty() || self.spawn_terminal() {
            return true;
        }
        self.state.terminal.open = false;
        self.state.focus = dacode::focus::Focus::Editor;
        false
    }

    /// Shows tab `index` and gives it the keys. Its rows are copied afresh for the next frame:
    /// the screen held is the last tab's.
    fn switch_terminal(&mut self, index: usize) {
        if index >= self.terminals.len() {
            return;
        }
        if index != self.state.terminal.active {
            self.state.terminal.active = index;
            self.state.terminal.screen = None;
            self.terminal_gesture = None;
        }
        self.show_terminal(true);
    }

    /// The next tab (`delta` 1) or the previous (-1), wrapping round as tabs do everywhere.
    fn step_terminal(&mut self, delta: isize) {
        let count = self.terminals.len();
        if count == 0 {
            self.show_terminal(true);
            return;
        }
        let next = (self.state.terminal.active as isize + delta).rem_euclid(count as isize);
        self.switch_terminal(next as usize);
    }

    /// Ends tab `index`'s shell and closes the tab. Dropping the terminal hangs its shell up on
    /// the shell's own thread, so this never waits for it.
    fn kill_terminal(&mut self, index: usize) {
        if index >= self.terminals.len() {
            return;
        }
        drop(self.terminals.remove(index));
        self.terminal_removed(index);
    }

    /// The bookkeeping after the terminal at `index` has gone, killed or exited. The shown tab
    /// stays the same shell when it was another one; when it was this one, its right-hand
    /// neighbour slides into its place, as closing an editor tab does. The last one going hides
    /// the panel and hands the keys back — its height is kept, for the next one.
    fn terminal_removed(&mut self, index: usize) {
        let panel = &mut self.state.terminal;
        if panel.active > index {
            panel.active -= 1;
        } else if panel.active == index {
            panel.active = index.min(self.terminals.len().saturating_sub(1));
            panel.screen = None;
            self.terminal_gesture = None;
        }
        if self.terminals.is_empty() {
            let panel = &mut self.state.terminal;
            panel.open = false;
            panel.active = 0;
            panel.screen = None;
            if self.state.focus == dacode::focus::Focus::Terminal {
                self.state.focus = dacode::focus::Focus::Editor;
            }
        }
        self.sync_terminal_tabs();
    }

    /// The tabs' titles, from the shells. One lock per shell, so bounded by the tabs open.
    fn sync_terminal_tabs(&mut self) {
        self.state.terminal.tabs = self
            .terminals
            .iter()
            .map(|terminal| dacode::terminal::tab_title(terminal.title(), terminal.shell()))
            .collect();
    }

    /// `ctrl-``: shows the terminal and gives it the keys; from the terminal, hides it and gives
    /// them back; with it shown but the keys elsewhere, gives it the keys. One key both ways, so
    /// the key that took you to the shell is the one that takes you back.
    fn toggle_terminal(&mut self) {
        use dacode::focus::Focus;
        if !self.state.terminal.open {
            self.show_terminal(true);
        } else if self.state.focus == Focus::Terminal {
            self.hide_terminal();
        } else {
            self.state.focus = Focus::Terminal;
        }
    }

    /// Hides the panel. The shells keep running, as they do when VSCode's panel is closed: hiding
    /// a view is not a decision to kill what is running in it.
    fn hide_terminal(&mut self) {
        self.state.terminal.open = false;
        self.terminal_gesture = None;
        if self.state.focus == dacode::focus::Focus::Terminal {
            self.state.focus = dacode::focus::Focus::Editor;
        }
    }

    /// Tells every shell the size of the grid on screen — the hidden tabs too, so a program in one
    /// has already redrawn for the panel it will be shown in. Constant work per shell and a no-op
    /// when nothing changed, so it is simply asked whenever the loop looks.
    fn fit_terminal(&mut self) {
        // Folded, the panel has no rows to speak of, and telling a shell it has one would make
        // every program in it redraw for a strip, then again when it unfolds.
        if self.state.terminal_folded() {
            return;
        }
        let Some(rects) = self.terminal_rects() else {
            return;
        };
        for terminal in &mut self.terminals {
            terminal.resize(
                dacode_terminal::GridSize::new(rects.columns, rects.rows),
                (rects.cell_width as u16, rects.cell_height as u16),
            );
        }
    }

    /// The terminals' part of the idle step. Returns whether there is something new to draw.
    ///
    /// Reads the shown shell's dirty flag without clearing it: the screen is copied when a frame
    /// is drawn ([`App::refresh_terminal_screen`]), and the flag stays up until then, so however
    /// much output arrives between two frames the parser wakes the loop once. A hidden tab, or a
    /// hidden panel, draws nothing and so is never copied, which leaves its flag up and the loop
    /// asleep however busy that shell is.
    fn poll_terminal(&mut self) -> bool {
        let mut changed = false;
        let mut index = 0;
        while index < self.terminals.len() {
            let Some(exit) = self.terminals[index].exit() else {
                index += 1;
                continue;
            };
            // `exit` closes its tab, as it does in VSCode. A failure says so; a clean exit was
            // asked for and needs no message.
            self.terminals.remove(index);
            self.terminal_removed(index);
            if let Some(code) = exit.code.filter(|code| *code != 0) {
                self.state.hover = Some(format!("the terminal's shell exited with status {code}"));
            }
            changed = true;
        }
        if !self.state.terminal.open {
            return changed;
        }
        self.fit_terminal();
        changed
            || self
                .active_terminal()
                .is_some_and(dacode_terminal::Terminal::is_dirty)
    }

    /// Copies the shown terminal's visible rows for this frame, if they changed. Viewport-sized,
    /// whatever the scrollback holds. The titles and the colour-query palette are brought up to
    /// date here too: one small lock per tab.
    fn refresh_terminal_screen(&mut self) {
        if !self.state.terminal.open {
            return;
        }
        self.sync_terminal_tabs();
        let palette = dacode::terminal::palette(&self.state.theme);
        if self.terminal_palette != Some(palette) {
            for terminal in &self.terminals {
                terminal.set_palette(palette);
            }
            self.terminal_palette = Some(palette);
        }
        let Some(terminal) = self.active_terminal() else {
            return;
        };
        if terminal.is_dirty() || self.state.terminal.screen.is_none() {
            self.state.terminal.screen = Some(terminal.take_screen());
        }
    }

    /// A key while the terminal has focus. Everything goes to the shown shell except the few keys
    /// [`dacode::terminal::route`] keeps for the editor, which run the action it names.
    fn handle_terminal_key(&mut self, key: Key) -> bool {
        match dacode::terminal::route(key) {
            dacode::terminal::KeyRoute::Shell => {
                if let Some(terminal) = self.active_terminal() {
                    terminal.key(key);
                }
                false
            }
            dacode::terminal::KeyRoute::Editor(action) => self.run(action),
        }
    }

    /// Puts the shown terminal's selection on the clipboard.
    fn copy_terminal_selection(&mut self) {
        match self
            .active_terminal()
            .and_then(|terminal| terminal.selection_text())
        {
            Some(text) => dacode::clipboard::set(&text),
            None => {
                self.state.hover =
                    Some("nothing selected in the terminal — drag over text to select it".into());
            }
        }
    }

    /// Pastes the clipboard into the shown terminal, starting one if there is none.
    fn paste_into_terminal(&mut self) {
        if !self.show_terminal(true) {
            return;
        }
        if let (Some(terminal), Some(text)) = (self.active_terminal(), dacode::clipboard::get()) {
            terminal.paste(&text);
        }
    }

    /// The modifiers held, as a mouse report carries them.
    fn mouse_modifiers(&self) -> dacode_terminal::MouseModifiers {
        dacode_terminal::MouseModifiers {
            shift: self.modifiers.shift_key(),
            alt: self.modifiers.alt_key(),
            ctrl: self.modifiers.control_key(),
        }
    }

    /// A press on the terminal panel: its buttons, its tabs, its grid. Focus is already the
    /// terminal's — `route_click` decided that — except on the buttons that take it away.
    fn click_terminal(&mut self, panel: Rect, x: f32, y: f32) {
        let scale = self.scale();
        let rects = dacode::layout::terminal_panel(panel, &self.state.typography, scale);
        if dacode::layout::contains(rects.close, x, y) {
            self.hide_terminal();
            return;
        }
        if dacode::layout::contains(rects.new, x, y) {
            self.window_action(&Action::NewTerminal);
            return;
        }
        let tabs = dacode::layout::terminal_tabs(
            &rects,
            &self.state.terminal.tabs,
            &self.state.typography,
            scale,
        );
        for (index, tab) in tabs.iter().enumerate() {
            if dacode::layout::contains(tab.close, x, y) {
                self.kill_terminal(index);
                return;
            }
            if dacode::layout::contains(tab.tab, x, y) {
                self.switch_terminal(index);
                return;
            }
        }
        if !dacode::layout::contains(rects.header, x, y) {
            self.press_terminal_grid(&rects, x, y, dacode_terminal::MouseButton::Left);
        }
    }

    /// A press on the terminal's cells. Returns whether it was used.
    ///
    /// A program that asked for the mouse gets the press, and the drags and release after it;
    /// shift held selects anyway, xterm's convention for getting text out of `htop` or a `vim`
    /// with `mouse=a`. Otherwise the left button selects — by cell, word or line as the press is
    /// the first, second or third of a series — and shift extends the selection already there.
    fn press_terminal_grid(
        &mut self,
        rects: &dacode::layout::TerminalRects,
        x: f32,
        y: f32,
        button: dacode_terminal::MouseButton,
    ) -> bool {
        let modifiers = self.mouse_modifiers();
        let click_count = self.click_count;
        let Some(terminal) = self.active_terminal() else {
            return false;
        };
        let at = rects.cell_at(x, y);
        if terminal.mouse_modes().reporting() && !modifiers.shift {
            terminal.mouse(dacode_terminal::MouseEvent {
                button,
                action: dacode_terminal::MouseAction::Press,
                row: at.row,
                column: at.column,
                modifiers,
            });
            self.terminal_gesture = Some(TerminalGesture::Report {
                button,
                row: at.row,
                column: at.column,
            });
            return true;
        }
        if button != dacode_terminal::MouseButton::Left {
            return false;
        }
        if modifiers.shift && click_count == 1 && terminal.has_selection() {
            terminal.extend_selection(at);
        } else {
            let kind = match click_count {
                2 => dacode_terminal::SelectKind::Words,
                3 => dacode_terminal::SelectKind::Lines,
                _ => dacode_terminal::SelectKind::Cells,
            };
            terminal.select(kind, at);
        }
        self.terminal_gesture = Some(TerminalGesture::Select);
        true
    }

    /// The pointer moved with a button held during a terminal gesture: the edge follows it, the
    /// selection extends to it, or the program is told — once per cell, not per pixel.
    fn drag_terminal(&mut self) {
        let Some(gesture) = self.terminal_gesture else {
            return;
        };
        let (x, y) = self.pointer;
        let scale = self.scale();
        match gesture {
            TerminalGesture::Resize => {
                let layout = self.layout();
                if let Some(panel) = layout.terminal {
                    let shared = layout.editor.height + panel.height;
                    self.state.terminal.fraction =
                        dacode::layout::terminal_fraction_at(shared, panel.bottom(), y, scale);
                    self.fit_terminal();
                }
            }
            TerminalGesture::Select => {
                if let (Some(rects), Some(terminal)) =
                    (self.terminal_rects(), self.active_terminal())
                {
                    terminal.extend_selection(rects.cell_at(x, y));
                }
            }
            TerminalGesture::Report {
                button,
                row,
                column,
            } => {
                let Some(rects) = self.terminal_rects() else {
                    return;
                };
                let at = rects.cell_at(x, y);
                if (at.row, at.column) == (row, column) {
                    return;
                }
                let modifiers = self.mouse_modifiers();
                if let Some(terminal) = self.active_terminal() {
                    terminal.mouse(dacode_terminal::MouseEvent {
                        button,
                        action: dacode_terminal::MouseAction::Drag,
                        row: at.row,
                        column: at.column,
                        modifiers,
                    });
                }
                self.terminal_gesture = Some(TerminalGesture::Report {
                    button,
                    row: at.row,
                    column: at.column,
                });
            }
        }
    }

    /// A button came up. Ends the terminal gesture it started, telling the program if the press
    /// was the program's.
    fn release_terminal(&mut self, button: dacode_terminal::MouseButton) {
        match self.terminal_gesture {
            Some(TerminalGesture::Report {
                button: pressed, ..
            }) if pressed == button => {
                let modifiers = self.mouse_modifiers();
                let (x, y) = self.pointer;
                if let (Some(rects), Some(terminal)) =
                    (self.terminal_rects(), self.active_terminal())
                {
                    let at = rects.cell_at(x, y);
                    terminal.mouse(dacode_terminal::MouseEvent {
                        button,
                        action: dacode_terminal::MouseAction::Release,
                        row: at.row,
                        column: at.column,
                        modifiers,
                    });
                }
                self.terminal_gesture = None;
            }
            Some(TerminalGesture::Resize | TerminalGesture::Select)
                if button == dacode_terminal::MouseButton::Left =>
            {
                self.terminal_gesture = None;
            }
            _ => {}
        }
    }

    /// A right or middle press over the terminal's cells, for a program that asked for the mouse.
    /// Returns whether the program took it; a right press it did not take opens the menu.
    fn press_terminal_other(&mut self, button: dacode_terminal::MouseButton) -> bool {
        if self.state.menu.is_some()
            || self.state.palette.is_some()
            || self.state.help.is_some()
            || self.state.welcome.is_some()
        {
            return false;
        }
        let (x, y) = self.pointer;
        let Some(rects) = self.terminal_rects() else {
            return false;
        };
        if !dacode::layout::contains(rects.panel, x, y)
            || dacode::layout::contains(rects.header, x, y)
        {
            return false;
        }
        self.state.focus = dacode::focus::Focus::Terminal;
        self.press_terminal_grid(&rects, x, y, button)
    }

    /// Shows the up-and-down arrows over the panel's resizable edge and while it is being dragged,
    /// so the edge can be found by pointing at it. The window is told only when that changes.
    fn update_pointer_icon(&mut self) {
        let (x, y) = self.pointer;
        let resize = self.terminal_gesture == Some(TerminalGesture::Resize)
            || (self.state.menu.is_none()
                && self.state.palette.is_none()
                && self.state.help.is_none()
                && self.state.welcome.is_none()
                && self
                    .terminal_rects()
                    .is_some_and(|rects| dacode::layout::contains(rects.border, x, y)));
        if resize == self.resize_pointer {
            return;
        }
        self.resize_pointer = resize;
        if let Some(window) = &self.window {
            window.set_cursor(if resize {
                winit::window::CursorIcon::NsResize
            } else {
                winit::window::CursorIcon::Default
            });
        }
    }

    /// Handles a click, or a drag when `extend` is set.
    ///
    /// Reads the same geometry the chrome draws from, so a click lands where it looks like it should
    /// even in a three-way split. Focus is settled afterwards, as `press` does after a key, so a
    /// click that gave the tree or the results the keys closes a completion popup the text has
    /// stopped owning.
    fn click(&mut self, extend: bool) {
        // A click anywhere ends a Keys page row's wait for a key — clicking away is how every
        // "press a key" box is left — and a click on a command row starts another.
        if let Some(page) = &mut self.state.settings_page {
            page.cancel_capture();
        }
        self.route_click(extend);
        self.state.settle_focus();
    }

    /// Where a click goes, in the order [`App::click`] promises: the wizard, the overlays, then
    /// focus, then the chrome and the surfaces under it.
    fn route_click(&mut self, extend: bool) {
        if self.chosen_by_overlay {
            if extend || self.click_count > 1 {
                return;
            }
            self.chosen_by_overlay = false;
        }
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
        // The palette and the help panel are next, for the same reason, and before focus is
        // decided: choosing a row runs it exactly as Enter would, and Enter leaves the keys where
        // they were before the overlay opened. A drag starting on one reaches nothing behind it.
        if self.state.palette.is_some() && (extend || self.click_palette(x, y)) {
            return;
        }
        if self.state.help.is_some() {
            if !extend {
                self.click_help(x, y);
            }
            return;
        }
        // The bell's panel: its buttons run their commands, and the rest of the card is not a click
        // on whatever is under it.
        if !extend && self.state.chrome.show_notifications {
            let bounds = self.window_bounds();
            let panel =
                dacode::chrome::notifications_panel(&self.state, layout.title, bounds, scale);
            if dacode::layout::contains(panel.card, x, y) {
                let command = dacode::chrome::notifications_button_at(
                    &self.state,
                    layout.title,
                    bounds,
                    scale,
                    x,
                    y,
                );
                if let Some(action) = command.and_then(Action::from_name) {
                    let _ = self.run(action);
                }
                return;
            }
        }
        // The theme dropdown, like the menu, takes the click wherever it lands. Only the first press
        // of a series: the second press of a double-click on the control that opened it lands on the
        // control again, outside the card, and would close the list it had just opened.
        if self.dropdown_showing() {
            if !extend && self.click_count <= 1 {
                self.click_dropdown(x, y);
            }
            return;
        }

        // The terminal's top edge, before focus: grabbing it to resize is not a decision about
        // where the keys go, and half of it lies over the code, which would otherwise take them.
        if !extend
            && self
                .terminal_rects()
                .is_some_and(|rects| dacode::layout::contains(rects.border, x, y))
        {
            self.terminal_gesture = Some(TerminalGesture::Resize);
            return;
        }

        // A press decides where the keys go, before anything below handles it. The chrome's own
        // bars say `None`, so a toolbar button does not move focus; the buttons that open a view
        // set it themselves.
        if !extend {
            // The sidebar only takes the keys when it is showing something that uses them; a click
            // in the problems list or a placeholder leaves focus where it was.
            if let Some(focus) = dacode::focus::focus_for(&layout, x, y).filter(|focus| {
                *focus != dacode::focus::Focus::Sidebar || self.state.sidebar_takes_keys()
            }) {
                self.state.focus = focus;
            }
        }

        // The chrome's buttons, before anything else: they are the mouse's way to the commands that
        // otherwise need a shortcut, and the whole reason someone who chose the conventional editor
        // can use this without being told anything.
        if !extend && self.click_chrome(&layout, x, y, scale) {
            return;
        }
        // The branch on the status line opens the picker, as VSCode's does.
        if !extend
            && self
                .state
                .scm
                .status()
                .and_then(|status| {
                    dacode::layout::status_branch(
                        layout.status,
                        &status.head_name(),
                        &self.state.typography,
                        scale,
                    )
                })
                .is_some_and(|branch| dacode::layout::contains(branch, x, y))
        {
            let _ = self.run(Action::Git(dacode_input::Git::Checkout));
            return;
        }

        // The sidebar owns its own clicks: choosing a file in the tree is the whole reason someone
        // reaches for the mouse there.
        if let Some(sidebar) = layout.sidebar {
            if dacode::layout::contains(sidebar, x, y) {
                if !extend {
                    self.click_sidebar(sidebar, x, y, scale);
                }
                return;
            }
        }
        // The title and activity bars have nothing to drag into the text: a press that missed every
        // button there is a press on nothing.
        if dacode::layout::contains(layout.title, x, y)
            || dacode::layout::contains(layout.activity, x, y)
        {
            return;
        }

        if let Some(panel) = layout.results {
            if dacode::layout::contains(panel, x, y) && !extend {
                self.click_results(panel, x, y, scale);
                return;
            }
        }
        // The terminal's own presses; a drag from it reaches nothing behind.
        if let Some(panel) = layout.terminal {
            if dacode::layout::contains(panel, x, y) {
                if !extend {
                    self.click_terminal(panel, x, y);
                }
                return;
            }
        }

        // The settings tab covers the panes: a press there is the page's, and a drag from it
        // reaches nothing behind.
        if self.state.settings_showing() && dacode::layout::contains(layout.editor, x, y) {
            if !extend {
                self.click_settings_page(x, y);
            }
            return;
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

        // A click on the blame column asks for that line's commit. The row is found the way a click
        // in the code finds one — through the code grid, at the column's x clamped to the first
        // character — so the entry under the pointer and the line asked about cannot disagree.
        if !extend
            && rects
                .blame
                .is_some_and(|column| dacode::layout::contains(column, x, y))
        {
            let line = self.state.document.text().char_to_line(offset);
            self.show_commit(line);
            return;
        }

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
                    .set_selections(dacode_text::SelectionSet::column(text, origin, head));
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
                .set_selections(dacode_text::SelectionSet::single(Selection::new(
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
                .set_selections(dacode_text::SelectionSet::from_iter(selections, primary));
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
            // obviously broken-feeling thing about using DaCode with a mouse. Both go through the
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
    fn offset_at(&self, code: Rect, x: f32, y: f32, scale: f32) -> dacode_text::CharOffset {
        // The same arithmetic that placed the glyph the pointer is over. It used to be a second
        // copy, and the two already disagreed about the left pad — drawing used 12, this used 8 —
        // so a click landed most of a character out before any of the horizontal work existed.
        let grid = self.code_grid(code, scale);
        // The rows drawn, including the one sliding in at the bottom while the top one is part-scrolled.
        let height = grid.rows_drawn();
        self.state.display.offset_at_display_point(
            self.state.document.text(),
            self.state.scroll,
            height,
            dacode_text::DisplayPoint {
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
    fn code_grid(&self, code: Rect, scale: f32) -> dacode::layout::CodeGrid {
        let row_height = self.state.typography.row_height() * scale;
        let area = dacode::layout::CodeArea::new(
            code,
            self.state.sticky_heads(
                self.state.document.text(),
                self.state.scroll.line,
                // Only the focused pane pins anything, and this grid is only ever built for it.
                true,
            ),
            row_height,
        );
        dacode::layout::CodeGrid::new(
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
        // The same reading `draw_code` takes, so a click on a part-scrolled row hits that row.
        .with_sub_row(self.state.sub_row_px(row_height))
    }

    /// The focused pane's grid, for the wheel and the horizontal reveal.
    fn focused_grid(&self) -> dacode::layout::CodeGrid {
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

    /// Whether the pointer is over chrome that lights up under it — a button, a tab, a row.
    ///
    /// They change when the pointer arrives or leaves, so the frame has to be redrawn then; and only
    /// then, because repainting on every pointer move over the code would spend a frame per mouse
    /// event on a window where nothing changed.
    /// Whether the pointer is over the terminal's header, whose tabs and buttons light up under it.
    fn over_terminal_header(&self) -> bool {
        let (x, y) = self.pointer;
        self.terminal_rects()
            .is_some_and(|rects| dacode::layout::contains(rects.header, x, y))
    }

    fn over_toolbar(&self) -> bool {
        let (x, y) = self.pointer;
        let layout = self.layout();
        dacode::layout::contains(layout.tabs, x, y)
            || (self.state.settings_showing() && dacode::layout::contains(layout.editor, x, y))
            || dacode::layout::contains(layout.title, x, y)
            || dacode::layout::contains(layout.activity, x, y)
            || layout
                .sidebar
                .is_some_and(|sidebar| dacode::layout::contains(sidebar, x, y))
    }

    /// The tab bar, laid out for the buffers that are open now.
    /// The settings tab is in it, after the files, from the same list the drawing reads.
    fn tab_bar(&self, strip: Rect, scale: f32) -> dacode::layout::TabBar {
        let names = self.state.tab_names();
        let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
        dacode::layout::tab_bar(
            strip,
            &borrowed,
            self.state.active_tab(),
            &self.state.typography,
            scale,
        )
    }

    /// A click on any of the chrome's controls: the title bar, the activity bar, the tab bar.
    ///
    /// Every one runs a named command through the same path its key takes, so a button cannot drift
    /// from the shortcut its tooltip teaches.
    fn click_chrome(
        &mut self,
        layout: &dacode::layout::Layout,
        x: f32,
        y: f32,
        scale: f32,
    ) -> bool {
        let at = |rect: Rect| dacode::layout::contains(rect, x, y);

        if at(layout.title) {
            let bar = dacode::layout::title_bar(layout.title, scale, &self.state.typography);
            // A title opens its menu exactly as F10 opens the first: the menu then owns the keys
            // and the pointer until it closes. The overflow lists the menus that did not fit.
            let title = bar.menus.iter().position(|rect| at(*rect)).or_else(|| {
                bar.overflow
                    .filter(|rect| at(*rect))
                    .map(|_| bar.menus.len())
            });
            if let Some(index) = title {
                let menu = dacode::menu::Menu::from_title(index, bar.menus.len(), self.state.modal);
                self.state.open_menu(menu);
                return true;
            }
            let command = if bar.search.is_some_and(at) {
                Some("command_palette")
            } else if at(bar.bell) {
                Some("notifications")
            } else {
                None
            };
            if let Some(action) = command.and_then(Action::from_name) {
                let _ = self.run(action);
            }
            return true;
        }

        if at(layout.activity) {
            let hit = dacode::layout::activity_items(layout.activity, scale)
                .into_iter()
                .find(|(_, rect)| at(*rect))
                .map(|(item, _)| item);
            if let Some(item) = hit {
                self.choose_activity(item);
            }
            return true;
        }

        if at(layout.tabs) {
            let bar = self.tab_bar(layout.tabs, scale);
            // The close button is inside the tab, so it is tested first — otherwise closing a file
            // would switch to it instead, which is the more annoying of the two wrong answers.
            let settings = self.state.settings_tab();
            for tab in &bar.tabs {
                // The settings tab is not a buffer: its number is one the buffer list never hands
                // out, and it closes and shows itself without touching that list.
                if Some(tab.index) == settings {
                    if at(tab.close) {
                        self.close_settings_tab();
                        return true;
                    }
                    if at(tab.rect) {
                        self.open_settings_tab();
                        return true;
                    }
                    continue;
                }
                if at(tab.close) {
                    // Closing a file's tab is not choosing it: with the settings tab showing, the
                    // file switched to in the background must not take its place on screen.
                    let showing = self.state.settings_active;
                    self.request_close_buffer(tab.index);
                    self.state.settings_active = showing && self.state.settings_page.is_some();
                    return true;
                }
                if at(tab.rect) {
                    self.switch_to(tab.index);
                    return true;
                }
            }
            let buttons = [bar.home, bar.back, bar.forward, bar.split, bar.run];
            if let Some((command, _, _)) = dacode::layout::TAB_BAR_COMMANDS
                .iter()
                .zip(buttons)
                .find(|(_, rect)| at(*rect))
                .map(|(entry, _)| entry)
            {
                if let Some(action) = Action::from_name(command) {
                    let _ = self.run(action);
                }
            }
            return true;
        }
        false
    }

    /// Opens a view from the activity bar, or closes it when it is the one already showing.
    ///
    /// The sidebar shows one view at a time. The commands behind the icons each toggle their own
    /// panel, so on their own two could be "open" at once with one hidden behind the other — and
    /// closing the visible one would then reveal a panel the user had left minutes ago.
    fn choose_activity(&mut self, item: dacode::layout::ActivityItem) {
        // The gear opens a tab rather than a view of the sidebar, so it leaves the sidebar alone.
        if item.activity == dacode::layout::Activity::Settings {
            if let Some(action) = Action::from_name(item.command) {
                let _ = self.run(action);
            }
            return;
        }
        if self.state.active_activity() == Some(item.activity) {
            self.state.close_sidebar();
            if item.activity == dacode::layout::Activity::Search {
                self.state.results = None;
            }
            return;
        }
        self.state.close_sidebar();
        if let Some(action) = Action::from_name(item.command) {
            let _ = self.run(action);
        }
    }

    /// The pointer moved while a menu is open. The row under it lights up, and over another title
    /// of the bar that menu opens — sweeping along the bar walks it, as VSCode's does. Returns
    /// whether anything changed, so an idle wobble does not cost a frame.
    fn move_over_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        let (bounds, scale, modal) = (self.window_bounds(), self.scale(), self.state.modal);
        let typography = self.state.typography;
        let Some(menu) = &mut self.state.menu else {
            return false;
        };
        let geometry = menu.geometry(bounds, scale, &typography);
        if menu.hover_title(&geometry, x, y, modal) {
            return true;
        }
        match menu.row_at(&geometry, x, y) {
            Some(index) if menu.selected != index => {
                menu.selected = index;
                true
            }
            _ => false,
        }
    }

    /// The palette or help row under the state's pointer — what the drawing lights.
    fn overlay_row_at(&self) -> Option<usize> {
        let (x, y) = self.state.pointer;
        if let Some(geometry) = self.palette_geometry() {
            return Palette::row_at(&geometry, x, y);
        }
        dacode::help::Help::line_at(&self.help_geometry()?, x, y)
    }

    /// Where the open palette is: the same function and the same anchor the drawing reads.
    fn palette_geometry(&self) -> Option<dacode::palette::Geometry> {
        let (bounds, scale) = (self.window_bounds(), self.scale());
        let anchor = dacode::palette::anchor(bounds, scale, &self.state.typography);
        Some(self.state.palette.as_ref()?.geometry(bounds, anchor, scale))
    }

    /// Where the open help panel is, as the drawing places it.
    fn help_geometry(&self) -> Option<dacode::help::Geometry> {
        Some(
            self.state
                .help
                .as_ref()?
                .geometry(self.window_bounds(), self.scale()),
        )
    }

    /// A click while the palette is open. Returns whether the click is used up; one outside the
    /// card closes it and goes on to whatever is under the pointer.
    fn click_palette(&mut self, x: f32, y: f32) -> bool {
        let Some(geometry) = self.palette_geometry() else {
            return false;
        };
        if let Some(index) = Palette::row_at(&geometry, x, y) {
            if let Some(palette) = &mut self.state.palette {
                palette.selected = index;
            }
            self.chosen_by_overlay = true;
            // Exactly what Enter does, through the same function — so a click cannot differ from
            // the key in anything but how it was asked for.
            self.handle_palette_key(Key::named(KeyCode::Enter));
            return true;
        }
        // The query field, a heading, the padding: part of the card, and nothing to do. So is the
        // command centre it hangs from: a click there fell through to the box's own button, which
        // opened a fresh command palette over whatever was being searched and threw the query away.
        let anchor =
            dacode::palette::anchor(self.window_bounds(), self.scale(), &self.state.typography);
        if dacode::layout::contains(geometry.card, x, y)
            || anchor.is_some_and(|anchor| dacode::layout::contains(anchor, x, y))
        {
            return true;
        }
        self.state.close_palette();
        false
    }

    /// A click while the help panel is open. It is a dialog over a scrim, so every click is its
    /// own: one outside the card only closes it.
    fn click_help(&mut self, x: f32, y: f32) {
        let Some(geometry) = self.help_geometry() else {
            return;
        };
        if let Some(line) = dacode::help::Help::line_at(&geometry, x, y) {
            let Some(help) = &mut self.state.help else {
                return;
            };
            // A heading is a row too, and runs nothing.
            if help.command_on(line).is_some() {
                help.selected = line;
                self.chosen_by_overlay = true;
                self.handle_help_key(Key::named(KeyCode::Enter));
            }
            return;
        }
        if !dacode::layout::contains(geometry.card, x, y) {
            self.state.close_help();
        }
    }

    /// A click while a menu is open.
    fn click_menu(&mut self, x: f32, y: f32, scale: f32) {
        let bounds = self.window_bounds();
        let modal = self.state.modal;

        let typography = self.state.typography;
        let Some(menu) = &mut self.state.menu else {
            return;
        };
        let geometry = menu.geometry(bounds, scale, &typography);

        // A context menu has no titles of its own, but the bar's are on screen beside it: a press
        // on one means that menu, not "close this one and then press again".
        if menu.kind == dacode::menu::Kind::Context {
            let bar = dacode::layout::title_bar(
                dacode::layout::title_strip(bounds, scale),
                scale,
                &typography,
            );
            let at = |rect: Rect| dacode::layout::contains(rect, x, y);
            let title = bar.menus.iter().position(|rect| at(*rect)).or_else(|| {
                bar.overflow
                    .filter(|rect| at(*rect))
                    .map(|_| bar.menus.len())
            });
            if let Some(index) = title {
                let menu = dacode::menu::Menu::from_title(index, bar.menus.len(), modal);
                self.state.open_menu(menu);
                return;
            }
        }

        // Pressing the open title again closes it, as a menu bar does everywhere; any other title
        // opens its own menu, and "…" its list.
        if let Some(index) = menu.title_at(&geometry, x, y) {
            if menu.is_open_title(&geometry, index) {
                self.state.close_menu();
            } else {
                menu.choose_title(index, geometry.titles.len(), modal);
            }
            return;
        }

        // A row of the overflow's list opens its menu in place, under "…".
        let row = menu.row_at(&geometry, x, y);
        if row.is_some_and(|row| menu.follow(row, modal)) {
            return;
        }
        let chosen = row
            .and_then(|index| menu.items.get(index))
            .map(|item| item.action.clone());

        // The card is padded, and the pointer sits on its top-left corner the instant a context
        // menu opens — inside the card, above the first row. Treating that strip as "clicked away"
        // meant a right-click followed by a left-click without moving the mouse dismissed the menu
        // and did nothing, which is the first thing anybody tries.
        if chosen.is_none() && dacode::layout::contains(geometry.card, x, y) {
            return;
        }

        self.state.close_menu();
        if let Some(action) = chosen {
            self.chosen_by_overlay = true;
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

    /// A click in the sidebar: the tree, the problems list, or nothing.
    fn click_sidebar(&mut self, sidebar: Rect, x: f32, y: f32, scale: f32) {
        // The row heights each panel draws with. Kept beside the drawing code's own constants; a
        // mismatch here shows up as clicking one row and selecting its neighbour.
        if let Some(tree) = &self.state.tree {
            // The same rectangles the drawing code uses, from the same function. This kept its own
            // copy of the walk, and the copies had already drifted: drawing left a pad's width free
            // at the bottom of the panel and this did not, so the two asked for different numbers
            // of visible rows. Whenever the tree was longer than the panel that is a different
            // *window* of rows, and clicking one file opened another.
            let panel = dacode::tree::panel(sidebar, scale, tree);
            if y >= panel.header.y && y < panel.header.bottom() {
                let _ = self.run(Action::NewFile);
                return;
            }
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
                self.state.focus = dacode::focus::Focus::Editor;
            }
            return;
        }

        if self.state.search_showing() {
            self.click_search_panel(sidebar, x, y, scale);
            return;
        }
        if self.state.source_control_showing() {
            self.click_source_control(sidebar, x, y, scale);
            return;
        }

        // The problems list. Choosing a row goes there — the panel exists to decide where to go
        // next, and one you can only read is half a panel.
        if self.state.show_diagnostics {
            let pad = space::LG * scale;
            let line = 20.0 * scale;
            let top = sidebar.y + pad + line * 1.4 + line * 1.6;

            let problems = self.state.problems.ordered().to_vec();

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
        }
    }

    /// Where everything on the settings tab is, as the last frame drew it: the editor area, laid out
    /// at the scroll the spring had reached by then. The one geometry the mouse reads, and the one
    /// `draw_settings_page` draws from.
    fn settings_page_geometry(&self) -> Option<dacode::settings_ui::PageGeometry> {
        let page = self
            .state
            .settings_page
            .as_ref()
            .filter(|_| self.state.settings_active)?;
        let now = self.last_frame.unwrap_or_else(std::time::Instant::now);
        let offset = self.state.chrome.motion.movement(
            dacode::motion::Key::SettingsScroll,
            page.scroll(),
            now,
        );
        Some(page.geometry_at(
            offset,
            self.layout().editor,
            self.scale(),
            &self.state.setting_values(),
            self.state.modal,
            &self.state.typography,
        ))
    }

    /// Where the theme dropdown is, as the last frame drew it: hung from the theme row of
    /// [`App::settings_page_geometry`], inside the editor area. The one geometry the mouse reads and
    /// the one `chrome` draws the card from.
    fn settings_dropdown_geometry(&self) -> Option<dacode::settings_ui::DropdownGeometry> {
        let page = self.state.settings_page.as_ref()?;
        page.dropdown()?;
        let geometry = self.settings_page_geometry()?;
        page.dropdown_geometry(
            &geometry,
            self.layout().editor,
            self.scale(),
            &self.state.typography,
        )
    }

    /// Whether the theme dropdown is open over a settings tab on screen: an overlay, which takes the
    /// keys and the clicks before the page under it.
    fn dropdown_showing(&self) -> bool {
        self.state.settings_showing()
            && self
                .state
                .settings_page
                .as_ref()
                .is_some_and(|page| page.dropdown().is_some())
    }

    /// Opens the theme dropdown on the theme in use, with the row it hangs from scrolled into view
    /// first so the card has something to hang from.
    fn open_theme_dropdown(&mut self) {
        let (rect, scale, modal) = (self.layout().editor, self.scale(), self.state.modal);
        let typography = self.state.typography;
        let current = self.state.setting_values().theme;
        if let Some(page) = &mut self.state.settings_page {
            page.reveal(rect, scale, modal, &typography);
            page.open_dropdown(current);
        }
        self.reveal_dropdown();
    }

    fn reveal_dropdown(&mut self) {
        let Some(geometry) = self.settings_page_geometry() else {
            return;
        };
        let (editor, scale, typography) =
            (self.layout().editor, self.scale(), self.state.typography);
        if let Some(page) = &mut self.state.settings_page {
            page.reveal_dropdown(&geometry, editor, scale, &typography);
        }
    }

    fn scroll_dropdown(&mut self, pixels: f32) {
        let Some(geometry) = self.settings_page_geometry() else {
            return;
        };
        let (editor, scale, typography) =
            (self.layout().editor, self.scale(), self.state.typography);
        if let Some(page) = &mut self.state.settings_page {
            page.scroll_dropdown(pixels, &geometry, editor, scale, &typography);
        }
    }

    /// Closes the dropdown and applies `name`: a change on the page like any other, so it glows, is
    /// marked touched and is written after the pause.
    fn pick_theme(&mut self, name: &'static str) {
        if let Some(page) = &mut self.state.settings_page {
            page.close_dropdown();
        }
        self.change_setting(dacode::settings_ui::Field::Theme, |values| {
            values.theme = Some(name);
        });
    }

    /// Keys while the theme dropdown is open. ↑ ↓ (and page up and down, home and end) move the
    /// highlight, enter or space picks it, escape closes. Any other key closes the list and goes on
    /// where it would have gone, so `ctrl-shift-p` still opens the palette and a letter still types
    /// into the search box — a dropdown is not worth a key that does nothing.
    fn handle_dropdown_key(&mut self, key: Key) -> bool {
        let plain = !key.ctrl && !key.alt;
        let delta = match key.code {
            KeyCode::Up if plain => Some(-1),
            KeyCode::Down if plain => Some(1),
            KeyCode::PageUp if plain => Some(-8),
            KeyCode::PageDown if plain => Some(8),
            KeyCode::Home if plain => Some(isize::MIN / 2),
            KeyCode::End if plain => Some(isize::MAX / 2),
            _ => None,
        };
        if let Some(delta) = delta {
            if let Some(page) = &mut self.state.settings_page {
                page.move_dropdown(delta);
            }
            self.reveal_dropdown();
            return true;
        }
        let choice = self
            .state
            .settings_page
            .as_ref()
            .and_then(|page| page.dropdown_choice());
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') if plain => {
                match choice {
                    Some(name) => self.pick_theme(name),
                    None => self.close_dropdown(),
                }
                true
            }
            KeyCode::Escape => {
                self.close_dropdown();
                true
            }
            _ => {
                self.close_dropdown();
                false
            }
        }
    }

    fn close_dropdown(&mut self) {
        if let Some(page) = &mut self.state.settings_page {
            page.close_dropdown();
        }
    }

    /// A click while the dropdown is open, wherever it lands: on a theme it picks it, on a heading
    /// or the card's padding it does nothing, and anywhere else it closes the list — what clicking
    /// away from a dropdown means everywhere — without reaching what is under it.
    fn click_dropdown(&mut self, x: f32, y: f32) {
        let Some(geometry) = self.settings_dropdown_geometry() else {
            self.close_dropdown();
            return;
        };
        if let Some(index) = geometry.hit(x, y) {
            if let Some(name) = dacode::settings_ui::theme_picks()
                .get(index)
                .and_then(|pick| pick.theme())
            {
                // The pick closes the list, so the rest of a double-click would reach the page under
                // the card — and whichever control was there.
                self.chosen_by_overlay = true;
                self.pick_theme(name);
            }
        } else if !geometry.contains(x, y) {
            self.close_dropdown();
        }
    }

    /// A click on the settings tab. The tree opens a page; a control changes its setting exactly as
    /// → or space would, a number at its limit wrapping (a click has no other direction to go in);
    /// the ⟲ resets; a row is selected, and an import item, an action or the link is chosen.
    fn click_settings_page(&mut self, x: f32, y: f32) {
        use dacode::settings_ui::{Entry, Line, Part};
        let Some(geometry) = self.settings_page_geometry() else {
            return;
        };
        let modal = self.state.modal;
        let Some(page) = &self.state.settings_page else {
            return;
        };
        let searching = !page.query().trim().is_empty();
        let open = page.category();
        if let Some((category, _)) = geometry
            .tree
            .iter()
            .find(|(_, rect)| dacode::layout::contains(*rect, x, y))
        {
            // Folded, the one button names the page and moves to the next — or, over a search's
            // results from every page, ends the search and shows the page that was open.
            let to = match (geometry.folded, searching) {
                (true, true) => open,
                (true, false) => category.next(),
                (false, _) => *category,
            };
            self.open_settings_category(to);
            return;
        }
        let Some((index, part)) = geometry.hit(x, y) else {
            return;
        };
        let Some(page) = &mut self.state.settings_page else {
            return;
        };
        page.select(index, modal);
        let Some(entry) = page.selected_entry(modal) else {
            return;
        };
        match (part, entry) {
            (Part::Reset, Entry::Field(field)) => self.reset_setting(field),
            (Part::Reset, Entry::Command(command)) => self.restore_keys(command),
            (_, Entry::Command(command)) => self.start_key_capture(command),
            (Part::Decrease, Entry::Field(field)) => self.adjust_setting(field, -1),
            (Part::Increase, Entry::Field(field)) => self.adjust_setting(field, 1),
            (Part::Control, Entry::Field(field)) => self.choose_page_entry(Entry::Field(field)),
            // A setting's row away from its control only selects it, so a click to read its
            // explanation does not change it.
            (_, Entry::Field(_) | Entry::Heading { .. }) => {}
            (_, Entry::Line(Line::Import(item))) => self.tick_import(item),
            (_, entry) => self.choose_page_entry(entry),
        }
    }

    /// A click in the results panel selects and opens that hit.
    fn click_results(&mut self, panel: Rect, x: f32, y: f32, scale: f32) {
        let Some(results) = &mut self.state.results else {
            return;
        };
        // The rows the panel drew, from the function that drew them: this used to redo the header
        // and row arithmetic here, a second copy that had to agree with the drawing by luck.
        let geometry = results.geometry(panel, scale);
        if let Some(index) = dacode::results::Results::hit_at(&geometry, x, y) {
            results.selected = index;
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
    ///
    /// **A change made on the settings page and not yet written wins over the file.** The file is
    /// what the change is about to be written into, so reading it back first must not undo it —
    /// and that is the normal order when a held write is released: saving the open init.lua is
    /// reported by the watcher in the same idle step, ahead of the write. Without this the reload
    /// put the old value back and the write then pinned it. The pending fields are taken from the
    /// live editor before loading and applied again after.
    ///
    /// **Unless the file changed that same field: then the file wins.** The most recent intent is
    /// what applies. A reload that leaves a pending field where the file last had it — the save
    /// releasing a held write, an edit to another line — says nothing about that field, so the page's
    /// change stands. One that moves it is a newer edit than the page's, made by hand while the
    /// change waited, and re-applying the page over it reverted that edit live and on disk. The
    /// field is then no longer pending, and nothing writes it back.
    fn reload_config(&mut self) {
        let pending: Vec<dacode::settings_ui::Field> = self
            .settings_touched
            .iter()
            .chain(&self.settings_reset)
            .copied()
            .collect();
        let kept = self.state.setting_values();
        let before = self.file_values.clone();
        self.reload_config_from_file();
        let loaded = self.state.setting_values();
        for field in pending {
            if loaded.differs(field, &before) {
                self.settings_touched.retain(|touched| *touched != field);
                self.settings_reset.retain(|reset| *reset != field);
            } else {
                self.apply_setting(field, &kept);
            }
        }
        if self.settings_touched.is_empty() && self.settings_reset.is_empty() && !self.keys_touched
        {
            self.settings_changed_at = None;
            self.settings_blocked = false;
        }
        self.file_values = loaded;
    }

    fn reload_config_from_file(&mut self) {
        let settings = dacode::config::load_from(&self.config_dir);

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
        if self.state.blame.inline != settings.inline_blame {
            self.state.blame.inline = settings.inline_blame;
            self.blame_wanted = true;
        }
        self.set_transparency(settings.transparency);
        self.state.chrome.zoom = settings.interface_zoom;
        self.state.chrome.motion.set_animations(settings.animations);
        self.state.chrome.updates = settings.updates;
        self.grammar.set_keymap(settings.keymap);
        // A key moved on the page and not yet written wins over the file it is about to be written
        // into, as a pending setting does.
        if !self.keys_touched {
            dacode_input::shortcuts::install(settings.shortcuts);
        }

        // Servers already running are left alone — a reload is not a reason to throw away minutes
        // of rust-analyzer's indexing. The new table takes effect the next time one is started,
        // and the settings tab's report is refreshed so it does not describe the file as it was.
        self.configured_servers = settings.servers;
        self.configured_formatters = settings.formatters;
        self.missing_servers.clear();
        if self.state.settings_page.is_some() {
            let servers = self.detect_servers();
            let formatters = self.detect_formatters();
            let modal = self.state.modal;
            if let Some(page) = &mut self.state.settings_page {
                page.set_reports(servers, formatters, modal);
            }
        }

        // Wrap width is in columns and follows the font size, which may just have changed.
        if let Some(window) = &self.window {
            let scale = self.scale();
            let width = window.inner_size().width as f32;
            fit_wrap_columns(&mut self.state, width, scale);
        }

        self.state.hover = Some(if settings.problems.is_empty() {
            "config reloaded".to_string()
        } else {
            for problem in &settings.problems {
                eprintln!("dacode: config: {problem}");
            }
            std::iter::once(format!("config: {} problems", settings.problems.len()))
                .chain(settings.problems.iter().cloned())
                .collect::<Vec<_>>()
                .join("\n")
        });
        eprintln!("dacode: reloaded configuration");
    }

    /// Starts watching the active file, if it has one.
    fn watch_active(&mut self) {
        let Some(path) = self.state.file_path.clone() else {
            return;
        };
        if let Some(watcher) = &mut self.watcher {
            if let Err(error) = watcher.watch(std::path::Path::new(&path)) {
                eprintln!("dacode: could not watch {path}: {error}");
            }
        }
    }

    /// Watches every directory the project's files live in, so a new file in one is noticed.
    ///
    /// Taken from the walk's own result rather than from the root, which means `.gitignore` has
    /// already decided what is watched: a recursive watch on the root would register one on every
    /// directory under `target/` as well — thousands of them against a per-user limit shared with
    /// the rest of the machine, and an event storm on every build.
    fn watch_project_directories(&mut self) {
        let directories: std::collections::BTreeSet<std::path::PathBuf> = self
            .index
            .files()
            .iter()
            .filter_map(|file| file.absolute.parent().map(std::path::Path::to_path_buf))
            .collect();

        if let Some(watcher) = &mut self.watcher {
            if !watcher.watch_project(directories) {
                eprintln!(
                    "dacode: not every directory could be watched; files added outside the editor may \
                     not appear until the folder is opened again"
                );
            }
        }
    }

    /// Starts another walk when something appeared or disappeared in the project.
    ///
    /// The index is built by one walk, and until this existed nothing ever asked for a second — so a
    /// file created by any other program, a `git checkout`, or a browser download was invisible to
    /// the tree and the file picker until the folder was opened again.
    ///
    /// A whole walk rather than an incremental patch, because the walk is what applies `.gitignore`,
    /// and it already runs on a background thread and reports through a poll. One walk covers a
    /// hundred new files as cheaply as one, which is what makes a branch switch cheap; and a walk
    /// already in flight is left to finish rather than restarted, since it will see everything that
    /// has landed by the time it gets there.
    fn rewalk_if_project_changed(&mut self) -> bool {
        let changed = self
            .watcher
            .as_mut()
            .is_some_and(|watcher| watcher.project_changed());

        if !changed || self.walk.is_some() {
            return false;
        }
        self.walk = Some(FileWalk::start(self.index.root().to_path_buf()));
        false
    }

    /// Handles files that changed outside the editor.
    ///
    /// The rule is that unsaved work is never discarded. A clean buffer is reloaded silently, which
    /// is what makes `git checkout` and a formatter run in a terminal behave the way you expect. A
    /// buffer with unsaved changes is *not* touched — it says so instead, and the choice of whose
    /// version wins stays with the person who made both.
    fn handle_file_changes(&mut self) -> bool {
        let Some(watcher) = &mut self.watcher else {
            return false;
        };
        let changed = watcher.poll();
        if changed.is_empty() {
            return false;
        }
        self.handle_changed_paths(changed)
    }

    /// What the watcher reported, acted on. Split from the poll so a test can hand it a path.
    fn handle_changed_paths(&mut self, changed: Vec<std::path::PathBuf>) -> bool {
        let mut redraw = false;
        let config_file = self.config_dir.join("init.lua");

        for path in changed {
            // Config first: reloading it is not reloading text.
            if dacode_project::canonical(&path) == dacode_project::canonical(&config_file) {
                // The settings tab's own write comes back here too. The live editor already *is*
                // that file, so reloading it would only re-walk PATH for the report, reset the
                // keymap and say "config reloaded" after every change — and revert a setting
                // changed since, before its own write. Recognised by content rather than by
                // `self_written`, which a user's save of an `init.lua` buffer also marks, and that
                // save must still apply.
                let ours = self.page_wrote.is_some()
                    && std::fs::read_to_string(&path).ok() == self.page_wrote;
                if ours {
                    self.self_written.remove(&path);
                    continue;
                }
                self.reload_config();
                redraw = true;
                // On to the buffer below: an outside edit of an `init.lua` that is also open
                // reloads that buffer or warns about it, as it would for any other file.
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
                .map(|open| dacode_project::canonical(std::path::Path::new(open)));
            if active.as_deref() != Some(path.as_path()) {
                continue;
            }

            if self.state.document.is_dirty() {
                // Told here, so the save this message offers is the one that overwrites — the
                // check in `write_active` would otherwise refuse it and say the same thing again.
                self.overwrite = Some(path.clone());
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
                        // Touched, or rewritten identically. Nothing to reload, but the stamp
                        // moved, and a stale one would refuse the next save for no reason.
                        if let Some(open) = self.state.file_path.clone() {
                            self.remember_disk(&open);
                        }
                        // The text is the same but the history may not be: a commit, a checkout or
                        // a rebase touches the file without changing what is in it.
                        self.blame_wanted = true;
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
                    if let Some(open) = self.state.file_path.clone() {
                        self.remember_disk(&open);
                    }
                    self.after_buffer_change();
                    let name = relative_crumbs(&path.to_string_lossy()).join("/");
                    eprintln!("dacode: reloaded {name} — changed on disk");
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
            dacode::tree::FileTree::new(self.index.root().to_path_buf(), self.index.files());
        if let Some(path) = &self.state.file_path {
            let absolute = dacode_project::canonical(std::path::Path::new(path));
            if let Ok(relative) = absolute.strip_prefix(self.index.root()) {
                tree.reveal(&relative.to_string_lossy());
            }
        }
        self.state.tree = Some(tree);
        self.state.show_diagnostics = false;
        self.state.focus = dacode::focus::Focus::Sidebar;
    }

    /// Switches the project to another folder: re-indexes, and shows the tree.
    ///
    /// The open buffers stay open. Changing what `ctrl-p` searches should not close the file you were
    /// reading when you decided to go looking for another one.
    fn open_folder(&mut self, folder: &std::path::Path) {
        // Through `canonical`, so what the statusline shows is a path a person recognises rather than
        // Windows' `\\?\C:\...` form.
        let root = dacode_project::canonical(folder);
        eprintln!("dacode: project root {}", root.display());

        self.index = FileIndex::new(root.clone());
        self.walk = Some(FileWalk::start(root.clone()));
        // Recorded as you open it rather than on the way out, so it survives a crash and so the next
        // launch lands here — which is the whole point of remembering it.
        dacode::session::remember_project(&dacode::session::session_dir(), &root);
        self.state.project_root = root;
        self.state.results = None;
        // The old folder's problems are about files that are no longer the project, and its search
        // results are hits in them: the query stays, searched again in the new one.
        self.state.problems.clear();
        let root = self.state.project_root.clone();
        if let Some(panel) = &mut self.state.search {
            panel.restart(&root);
        }
        // Another folder is another repository, or none: what the view knew is about the old one,
        // and a status for it still on its way is refused by the version `ask` moves on.
        self.state.scm.forget();
        self.branches.clear();
        self.refresh_scm(false);
        let kept = self.close_clean_buffers();
        self.show_tree();
        // Nothing to say when it simply worked: the tree changing is the answer, and a box over the
        // code announcing it was one more thing to dismiss after every folder change.
        if kept > 0 {
            self.state.hover = Some(format!(
                "{kept} file{} with unsaved changes kept open",
                if kept == 1 { "" } else { "s" }
            ));
        }
    }

    /// Closes every buffer with nothing unsaved in it, as a folder change does in any editor, and
    /// says how many were kept. Unsaved ones stay: closing them would lose work, and a dialog for
    /// each is more ceremony than reopening the old folder to save them.
    fn close_clean_buffers(&mut self) -> usize {
        // Highest first: closing one shifts only the numbers above it.
        for index in (0..=self.buffers.len()).rev() {
            if !self.buffer_is_dirty(index) {
                self.close_buffer(index);
            }
        }
        (0..=self.buffers.len())
            .filter(|index| self.buffer_is_dirty(*index))
            .count()
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
        let absolute = dacode_project::canonical(std::path::Path::new(&path));
        if let Ok(relative) = absolute.strip_prefix(&root) {
            tree.reveal(&relative.to_string_lossy());
        }
    }

    /// Resets per-buffer derived state after the active document changes.
    fn after_buffer_change(&mut self) {
        // The focused pane shows whatever is live. Without this it keeps drawing the buffer it
        // remembers, which is how choosing a file from the tree came to display an empty one.
        self.panes.follow(self.state.active_buffer);
        self.ensure_preview(false);

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
        // The tree too: its version is a number in the *old* document's sequence, and a new
        // document can reach the same number while holding different text.
        self.state.structure = None;
        // And the expansion history, whose offsets are into the old document.
        self.state.expansions.clear();
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
            match SyntaxWorker::spawn_with(language, self.waker.clone()) {
                Ok(worker) => self.syntax = Some(worker),
                Err(err) => eprintln!("dacode: syntax unavailable: {err}"),
            }
        }

        // The problems shown are the new file's.
        self.state
            .problems
            .set_current(self.state.file_path.as_deref().map(std::path::Path::new));

        // The diff describes the document that just left — unless this one is itself a diff, whose
        // hunks `]c` steps between as it does a file's.
        self.state.diff = self.review_marks();
        self.diffed_version = None;
        // So does the blame, and its line numbers mean nothing in the new one.
        self.state.blame.clear();
        self.blame_wanted = true;

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
        self.draw_frame();
        // The frame's instant belongs to the frame. Left set, `state.now()` would answer with the time
        // of the last draw for every handler that runs between frames, and a spring retargeted from a
        // keypress after an idle minute would start a minute in the past and arrive before it began.
        self.state.chrome.now = None;
    }

    fn draw_frame(&mut self) {
        // A redraw can be delivered before the idle step that would have done this.
        self.settle_scroll_input();
        // Also here, so the index and any server replies land without waiting for a keystroke.
        self.poll_walk();
        self.sync_lsp();
        // And the targets, for a redraw delivered before the idle step: a frame must never draw a view
        // that has opened at the place it was closed. At the frame's own instant, so the retarget and
        // what is drawn and what the mouse later measures are one moment.
        let now = std::time::Instant::now();
        self.sync_motion(now);
        // The terminal's grid follows the panel, and its rows are copied once for this frame.
        self.fit_terminal();
        self.refresh_terminal_screen();

        let Some(renderer) = &mut self.renderer else {
            return;
        };
        // Minimised: there is nothing to draw into, and building a frame for it is the same waste
        // as submitting one.
        if !renderer.is_visible() {
            return;
        }

        // Every animated value this frame draws is read at this one instant, so two things that move
        // together cannot be sampled a few microseconds apart.
        self.state.chrome.now = Some(now);
        self.last_frame = Some(now);

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
        // `scale()` spelled out: the renderer is borrowed. Keep the two the same.
        let scale = self
            .window
            .as_ref()
            .map(|window| window.scale_factor() as f32)
            .unwrap_or(1.0)
            * self.state.chrome.zoom.factor();
        // Opening or closing the sidebar changes how wide the code is, and any action can do that.
        // Constant work, so it is simply asked every frame rather than at each of those actions.
        fit_wrap_columns(&mut self.state, width as f32, scale);

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
                chrome::welcome_arrival(&self.state.chrome.motion, welcome, now),
            );
            if let Err(err) = renderer.render(&self.frame, welcome.preview_theme().bg) {
                eprintln!("dacode: frame dropped: {err:#}");
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
            eprintln!("dacode: frame dropped: {err:#}");
        }
    }
}

#[cfg(test)]
mod command_line_tests {
    use super::*;

    /// This file, so the tests below can ask what the binary actually reads rather than what
    /// somebody remembered to write down. Both directions matter and neither is checkable any other
    /// way: the flags are parsed a thousand lines from where they are listed.
    const SOURCE: &str = include_str!("main.rs");

    /// The three shapes a flag is read in: an equality against an argument, and the two closures
    /// [`screenshot`] builds over its argument list.
    const READ_AS: [&str; 3] = ["arg == \"", "flag(\"", "value(\""];

    /// The file with its comment lines removed.
    ///
    /// The first version of this scanned for `"--` anywhere and failed against the *prose* above
    /// it — a doc comment naming a flag is not the binary reading one. Stripping comments first,
    /// and then matching only the shapes in [`READ_AS`], means the set can be satisfied one way:
    /// by code that actually reads the argument.
    fn code() -> String {
        SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every flag reached by `pattern`, e.g. every one whose value is consumed.
    fn flags_read_by(code: &str, pattern: &str) -> Vec<String> {
        let mut found: Vec<String> = code
            .match_indices(pattern)
            .filter_map(|(index, _)| code[index + pattern.len()..].split('"').next())
            .filter(|name| name.starts_with("--") && name.len() > 2)
            .map(str::to_string)
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Every flag the binary reads, however it reads it.
    fn flags_in_source() -> Vec<String> {
        let code = code();
        let mut found: Vec<String> = READ_AS
            .iter()
            .flat_map(|pattern| flags_read_by(&code, pattern))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    #[test]
    fn every_flag_that_consumes_a_value_is_listed_as_one() {
        // The defect this is made of: `--row`, `--symbols` and `--block` all take an argument and
        // none was in VALUE_FLAGS, so `--row 12` fed `12` to `positional` and opened an empty buffer
        // called `12` behind the panel it was meant to be scrolling. Nothing failed and the panel
        // still drew, so rendering every overlay in CI could not see it either.
        //
        // The value closure is the only way a flag consumes the next argument, which makes the set
        // derivable rather than remembered.
        for name in flags_read_by(&code(), READ_AS[2]) {
            assert!(
                VALUE_FLAGS.contains(&name.as_str()),
                "{name} reads the argument after it but is not in VALUE_FLAGS, so its value will be \
                 opened as a file"
            );
        }
    }

    #[test]
    fn a_flag_offered_for_ordinary_use_works_without_screenshot() {
        // `--no-vim` sat in the ordinary group and was read only inside `screenshot`, so --help
        // advertised a way to start the editor without Vim keys that did nothing whatever: the
        // setting comes from the config, and the flag was never consulted. The other guards could
        // not see it — the flag *is* read, just not anywhere a person following the help could
        // reach. `screenshot` is the last item in the file before the event loop, so everything a
        // plain run can reach is the text above it.
        let code = code();
        let boundary = code
            .find("fn screenshot(")
            .expect("the headless renderer moved; this split needs rewriting");
        let ordinary = &code[..boundary];

        for (flag, _, _) in &FLAGS[..PLAIN_FLAGS] {
            assert!(
                READ_AS
                    .iter()
                    .any(|pattern| flags_read_by(ordinary, pattern).contains(&flag.to_string())),
                "--help lists {flag} for ordinary use, but nothing outside the headless renderer \
                 reads it, so it does nothing unless --screenshot is also given"
            );
        }
    }

    #[test]
    fn the_scan_finds_the_flags_that_are_really_there() {
        // A guard derived from source is worth exactly what its scan is worth, and this one has
        // already been wrong once. Three flags it must find, read three different ways.
        let found = flags_in_source();
        for flag in ["--screenshot", "--light", "--row"] {
            assert!(found.contains(&flag.to_string()), "the scan missed {flag}");
        }
        assert!(
            !found.contains(&"--".to_string()),
            "the bare separator is not a flag"
        );
    }

    #[test]
    fn the_help_names_no_flag_the_binary_does_not_read() {
        let real = flags_in_source();
        for (flag, _, _) in FLAGS {
            assert!(
                real.contains(&flag.to_string()),
                "--help offers {flag}, which nothing in the binary reads"
            );
        }
    }

    #[test]
    fn every_flag_the_binary_reads_is_in_the_help() {
        // The direction that rots silently: a flag added for one screenshot, never written down,
        // and undiscoverable by anyone who did not add it. That is how --row, --symbols and --block
        // came to exist for months with CLAUDE.md as their only mention.
        for flag in flags_in_source() {
            assert!(
                FLAGS.iter().any(|(name, _, _)| *name == flag),
                "{flag} is read by the binary but --help does not mention it"
            );
        }
    }

    #[test]
    fn a_message_never_tells_you_to_press_nothing() {
        // `install_language_server` has no key in either grammar, on purpose, and the "no language
        // server" message interpolated one anyway — so the sentence a first-time user of a Python
        // project reads had a blank where the instruction was. Checked over every command rather
        // than that one, because the next keyless command will be interpolated by somebody too.
        for modal in [true, false] {
            for command in dacode_input::commands::COMMANDS {
                let phrase = how_to_run(command.name, modal);
                assert!(
                    !phrase.trim().is_empty(),
                    "`{}` produces an empty phrase with modal={modal}, so any message using it has \
                     a hole in it",
                    command.name
                );
                assert!(
                    !phrase.contains("()"),
                    "`{}` names the palette without saying how to open it: {phrase}",
                    command.name
                );
            }
        }

        // The case it was built for, spelled out: no key, so the palette is named instead.
        let phrase = how_to_run("install_language_server", true);
        assert!(
            phrase.contains("command palette") && phrase.contains("install a language server"),
            "got: {phrase}"
        );
    }

    #[test]
    fn the_version_is_the_crate_version() {
        // Not a tautology: it is the assertion that stops the two drifting when the help text is
        // next edited by hand, and `dacode --version` is what a bug report is asked for.
        assert!(usage().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn the_help_describes_every_flag_it_lists() {
        for (flag, _, description) in FLAGS {
            assert!(
                !description.trim().is_empty(),
                "{flag} is listed with nothing said about it"
            );
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
        use dacode_text::{CharOffset, Selection, SelectionSet};

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
        // so the title bar showed `crates\dacode\src\main.rs` where Linux showed four breadcrumbs — a
        // defect no test could fail on and only a screenshot could show.
        assert_eq!(
            relative_crumbs(r"crates\dacode\src\main.rs"),
            vec!["crates", "dacode", "src", "main.rs"]
        );
        assert_eq!(
            relative_crumbs("crates/dacode/src/main.rs"),
            vec!["crates", "dacode", "src", "main.rs"]
        );
    }

    #[test]
    fn a_long_path_keeps_only_its_last_few_segments() {
        // The title bar is one line. A path from the filesystem root would push the file name off it,
        // which is the one part that has to be visible.
        let crumbs = relative_crumbs("/home/someone/code/project/crates/dacode/src/main.rs");
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
        let dir =
            std::env::temp_dir().join(format!("dacode-wiring-{}-{label}", std::process::id()));
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
        let mut app = App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &dacode::config::Settings::default(),
            None,
        );
        // Never the user's own config: a test that changed a setting would otherwise write it.
        app.config_dir = dir.join("config");
        // Nor the real updates directory, whose stamp and skipped version are the user's.
        app.update_dir = dir.join("updates");
        app
    }

    /// Runs the editor's idle work until `done`, or gives up. Never spins forever: a wiring defect
    /// that hangs must fail the test rather than the suite.
    fn settle(app: &mut App, mut done: impl FnMut(&App) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            // What the event loop does when a worker wakes it: a real editor is woken, a test has
            // to look.
            app.collect_background();
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// With the tree open and the editor focused, ↓ moves the caret — it used to move the tree.
    #[test]
    fn arrows_follow_focus_not_whichever_panel_is_open() {
        let dir = scratch("focus");
        std::fs::write(dir.join("b.rs"), "x").unwrap();
        let mut app = editor(&dir, "a.rs", "one\ntwo\nthree\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        indexed(&mut app);
        app.show_tree();
        app.state.focus = dacode::focus::Focus::Editor;
        app.press(Key::named(KeyCode::Down));
        assert_eq!(app.state.cursor_point().line, 1, "the caret moved");

        app.state.focus = dacode::focus::Focus::Sidebar;
        let before = app.state.cursor_point();
        let row = app.state.tree.as_ref().unwrap().selected_row().cloned();
        app.press(Key::named(KeyCode::Down));
        assert_eq!(app.state.cursor_point(), before, "the tree took it");
        assert_ne!(
            app.state.tree.as_ref().unwrap().selected_row().cloned(),
            row,
            "and the tree's selection really moved"
        );
    }

    /// The Vim grammar had the same defect: `j` is also the tree's down key.
    #[test]
    fn normal_mode_j_moves_the_caret_while_the_tree_is_merely_open() {
        let dir = scratch("focus-vim");
        std::fs::write(dir.join("b.rs"), "x").unwrap();
        let mut app = editor(&dir, "a.rs", "one\ntwo\nthree\n");
        app.state.modal = true;
        app.grammar.set_modal(true);
        app.state.mode = Mode::Normal;
        app.show_tree();
        app.state.focus = dacode::focus::Focus::Editor;
        app.press(Key::char('j'));
        assert_eq!(app.state.cursor_point().line, 1, "the caret moved");
    }

    /// Waits for the background walk, so the tree has rows to move through. Without it the tree is
    /// empty and "the selection moved" is `None` against `None`.
    fn indexed(app: &mut App) {
        app.walk = Some(FileWalk::start(app.index.root().to_path_buf()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.index.files().is_empty() && std::time::Instant::now() < deadline {
            app.poll_walk();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!app.index.files().is_empty(), "the walk finished");
    }

    fn two_hits(dir: &std::path::Path) -> dacode::results::Results {
        let hit = |line_number| dacode_project::SearchHit {
            path: dir.join("a.rs"),
            line_number,
            line: "x".into(),
            range: 0..0,
        };
        dacode::results::Results::from_hits(dir, "x", vec![hit(1), hit(2)])
    }

    /// An open search used to take Enter and the arrows whatever the caret was doing, so a newline
    /// became "open the selected hit".
    #[test]
    fn results_take_keys_only_when_they_have_focus() {
        use dacode::focus::Focus;
        let dir = scratch("focus-results");
        let mut app = editor(&dir, "a.rs", "one\ntwo\nthree\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        app.state.show_results(two_hits(&dir));
        assert_eq!(
            app.state.focus,
            Focus::Panel,
            "showing results gives them the keys"
        );

        app.press(Key::named(KeyCode::Down));
        assert_eq!(
            app.state.results.as_ref().unwrap().selected,
            1,
            "the panel took it"
        );
        assert_eq!(app.state.cursor_point().line, 0);

        app.state.focus = Focus::Editor;
        app.press(Key::named(KeyCode::Down));
        assert_eq!(
            app.state.results.as_ref().unwrap().selected,
            1,
            "the panel did not"
        );
        assert_eq!(app.state.cursor_point().line, 1, "the caret did");

        let before = app.state.document.text().to_string();
        app.press(Key::named(KeyCode::Enter));
        assert_ne!(
            app.state.document.text().to_string(),
            before,
            "Enter typed a newline"
        );
        assert!(app.state.results.is_some(), "and opened no hit");
    }

    /// Escape from the panel closes it and hands the keys back in one press.
    #[test]
    fn escape_in_the_results_closes_them_and_returns_the_keys() {
        let dir = scratch("focus-results-escape");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.show_results(two_hits(&dir));
        app.press(Key::named(KeyCode::Escape));
        assert!(app.state.results.is_none());
        assert_eq!(app.state.focus, dacode::focus::Focus::Editor);
    }

    /// A click in a sidebar view that takes no keys must not leave focus there: Escape would be
    /// swallowed by a list that has no use for it.
    #[test]
    fn escape_reaches_the_editor_after_a_click_in_the_problems_list() {
        let dir = scratch("focus-problems");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.modal = true;
        app.grammar.set_modal(true);
        assert_eq!(app.state.mode, Mode::Insert);

        app.run(Action::ToggleDiagnosticsList);
        assert!(app.state.show_diagnostics);
        let layout = app.layout();
        let sidebar = layout.sidebar.expect("the list is a sidebar view");
        app.pointer = (sidebar.x + 20.0, sidebar.y + 100.0);
        app.click(false);

        app.press(Key::named(KeyCode::Escape));
        assert_eq!(app.state.mode, Mode::Normal, "Escape left insert mode");
    }

    /// Even a focus left over from a view that has since closed must not eat a key.
    #[test]
    fn a_stale_sidebar_focus_never_swallows_escape() {
        let dir = scratch("focus-stale");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.modal = true;
        app.grammar.set_modal(true);
        app.state.focus = dacode::focus::Focus::Sidebar;
        app.press(Key::named(KeyCode::Escape));
        assert_eq!(app.state.mode, Mode::Normal);
        assert_eq!(app.state.focus, dacode::focus::Focus::Editor);
    }

    /// Opening a file goes to it, as it does everywhere: the next `j` is typed, not a tree move.
    /// Expanding a folder is not opening a file, so it keeps the keys.
    #[test]
    fn opening_a_file_from_the_tree_returns_the_keys_to_it() {
        use dacode::focus::Focus;
        let dir = scratch("focus-open");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("c.rs"), "c").unwrap();
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        indexed(&mut app);
        app.show_tree();
        let tree = app.state.tree.as_mut().unwrap();
        tree.reveal("a.rs");
        assert_eq!(app.state.focus, Focus::Sidebar);
        app.press(Key::named(KeyCode::Enter));
        assert_eq!(app.state.focus, Focus::Editor, "a file was opened");

        app.state.focus = Focus::Sidebar;
        app.state.tree.as_mut().unwrap().reveal("sub");
        app.press(Key::named(KeyCode::Enter));
        assert_eq!(app.state.focus, Focus::Sidebar, "a folder only expanded");
    }

    /// The title bar is chrome: pressing it must not move the keys, through the real `click()`.
    #[test]
    fn a_click_on_the_title_bar_leaves_focus_alone() {
        use dacode::focus::Focus;
        let dir = scratch("focus-title");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.show_tree();
        assert_eq!(app.state.focus, Focus::Sidebar);
        let layout = app.layout();
        // The far left of the title bar, clear of the search box and every button.
        app.pointer = (layout.title.x + 2.0, layout.title.y + 2.0);
        app.click(false);
        assert_eq!(app.state.focus, Focus::Sidebar);
    }

    /// The menu bar through the real `click()` and the real pointer handler: a title opens its
    /// menu, sweeping to the next opens that one, and pressing the open title again closes it.
    #[test]
    fn a_title_in_the_bar_opens_its_menu_and_a_sweep_opens_the_next() {
        let dir = scratch("menu-bar-click");
        let mut app = editor(&dir, "a.rs", "one\n");
        let layout = app.layout();
        let bar = dacode::layout::title_bar(layout.title, app.scale(), &app.state.typography);
        let middle = |rect: Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);

        app.pointer = middle(bar.menus[0]);
        app.click(false);
        let menu = app.state.menu.as_ref().expect("a click on File opens it");
        assert_eq!(menu.kind, dacode::menu::Kind::Bar);
        assert_eq!(menu.open, 0);

        app.pointer = middle(bar.menus[1]);
        app.state.pointer = app.pointer;
        assert!(app.move_over_menu(), "moving onto Edit redraws");
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.open), Some(1));

        app.click(false);
        assert!(
            app.state.menu.is_none(),
            "pressing the open title again closes it"
        );

        app.pointer = middle(bar.menus[2]);
        app.click(false);
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.open), Some(2));
        app.pointer = (layout.editor.x + 300.0, layout.editor.y + 300.0);
        app.click(false);
        assert!(app.state.menu.is_none(), "a click away closes it");
    }

    /// A menu opened from the title bar takes the keys as one opened with F10 does — even from a
    /// results panel that holds focus, which used to be offered Enter and the arrows first.
    #[test]
    fn a_menu_opened_by_the_mouse_takes_the_keys_from_a_focused_panel() {
        use dacode::focus::Focus;
        let dir = scratch("menu-bar-keys");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.show_results(two_hits(&dir));
        assert_eq!(app.state.focus, Focus::Panel);
        let layout = app.layout();
        let bar = dacode::layout::title_bar(layout.title, app.scale(), &app.state.typography);
        app.pointer = (bar.menus[0].x + 4.0, bar.menus[0].y + 4.0);
        app.click(false);

        app.press(Key::named(KeyCode::Down));
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.selected), Some(1));
        assert_eq!(
            app.state.results.as_ref().map(|results| results.selected),
            Some(0),
            "the panel behind the menu did not move"
        );
        app.press(Key::named(KeyCode::Right));
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.open), Some(1));
        app.press(Key::named(KeyCode::Escape));
        assert!(app.state.menu.is_none());
        assert!(
            app.state.results.is_some(),
            "Escape closed the menu, not the panel under it"
        );
    }

    /// With completion showing, a click on a title used to open a menu the keys never reached:
    /// Down moved the hidden popup and Enter accepted it into the buffer, with Help still open.
    #[test]
    fn a_menu_opened_over_completion_takes_the_keys_and_the_buffer_is_untouched() {
        let dir = scratch("menu-over-completion");
        let mut app = editor(&dir, "a.rs", "alpha alphabet\nal");
        app.state.completion = Some(dacode::completion::Completion::new(
            app.state.document.text(),
            app.state.cursor(),
            app.state.document.version(),
        ));
        let before = app.state.document.text().to_string();
        let layout = app.layout();
        let bar = dacode::layout::title_bar(layout.title, app.scale(), &app.state.typography);
        let help = dacode::menu::TITLES
            .iter()
            .position(|t| *t == "Help")
            .unwrap();
        app.pointer = (bar.menus[help].x + 4.0, bar.menus[help].y + 4.0);
        app.click(false);

        app.press(Key::named(KeyCode::Down));
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.selected), Some(1));
        app.press(Key::named(KeyCode::Enter));
        assert_eq!(app.state.document.text().to_string(), before);
        assert!(app.state.menu.is_none());
        assert!(
            app.state.palette.is_some(),
            "Enter ran Help's second row, the command palette"
        );
    }

    /// A completion session for whatever is under the caret, as typing would have opened.
    fn completing(app: &mut App) {
        app.state.completion = Some(dacode::completion::Completion::new(
            app.state.document.text(),
            app.state.cursor(),
            app.state.document.version(),
        ));
        assert!(app.state.completion.is_some());
    }

    /// The popup belongs to the caret. Left open while the tree had the keys, it went on taking ↓
    /// and Enter from the tree that looked focused, and Enter accepted a suggestion into a buffer
    /// nobody was looking at — the arrows going to something other than what was clicked.
    #[test]
    fn with_completion_open_a_focused_tree_still_gets_the_arrows() {
        use dacode::focus::Focus;
        let dir = scratch("completion-tree");
        std::fs::write(dir.join("b.rs"), "x").unwrap();
        let mut app = editor(&dir, "a.rs", "alpha alphabet\nal");
        app.state.modal = false;
        app.grammar.set_modal(false);
        indexed(&mut app);
        app.show_tree();
        assert_eq!(app.state.focus, Focus::Sidebar);
        completing(&mut app);

        let before = app.state.document.text().to_string();
        let row = app.state.tree.as_ref().unwrap().selected_row().cloned();
        app.press(Key::named(KeyCode::Down));
        assert_ne!(
            app.state.tree.as_ref().unwrap().selected_row().cloned(),
            row,
            "the tree's selection moved"
        );
        assert_eq!(app.state.document.text().to_string(), before);
        assert!(
            app.state.completion.is_none(),
            "and the popup the text stopped owning is gone rather than drawn over the tree"
        );
    }

    /// A click is the other way focus leaves the text, and it must close the popup too: expanding
    /// a folder keeps the keys in the tree, so nothing else would.
    #[test]
    fn a_click_that_gives_the_tree_the_keys_closes_completion() {
        use dacode::focus::Focus;
        let dir = scratch("completion-tree-click");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("c.rs"), "c").unwrap();
        let mut app = editor(&dir, "a.rs", "alpha alphabet\nal");
        indexed(&mut app);
        app.show_tree();
        app.state.focus = Focus::Editor;
        completing(&mut app);

        let sidebar = app.layout().sidebar.expect("the tree is a sidebar view");
        let panel = dacode::tree::panel(sidebar, app.scale(), app.state.tree.as_ref().unwrap());
        let (_, rect) = panel
            .rows
            .iter()
            .find(|(row, _)| row.relative == "sub")
            .expect("the folder has a row");
        app.pointer = middle(*rect);
        app.click(false);
        assert_eq!(app.state.focus, Focus::Sidebar);
        assert!(app.state.completion.is_none());
    }

    /// Help is a card on the modal layer, so it owns the keys over anything merely focused. It was
    /// asked after a focused results panel, so ↓ moved the hits hidden under it and Enter opened
    /// one with the card still on top — and since Enter in help runs its row, the row never ran.
    #[test]
    fn help_over_a_focused_results_panel_takes_the_arrows_and_enter() {
        use dacode::focus::Focus;
        let dir = scratch("help-over-results");
        std::fs::write(dir.join("b.rs"), "bee\nbee\n").unwrap();
        let mut app = editor(&dir, "a.rs", "one\ntwo\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        let hit = |line_number| dacode_project::SearchHit {
            path: dir.join("b.rs"),
            line_number,
            line: "bee".into(),
            range: 0..3,
        };
        app.state.show_results(dacode::results::Results::from_hits(
            &dir,
            "bee",
            vec![hit(1), hit(2)],
        ));
        assert_eq!(app.state.focus, Focus::Panel);

        app.press(Key::named(KeyCode::Function(1)));
        assert!(app.state.help.is_some(), "F1 opened help over the panel");
        app.press(Key::named(KeyCode::Down));
        assert_eq!(
            app.state.results.as_ref().map(|results| results.selected),
            Some(0),
            "the hits under the card did not move"
        );

        // Filtered to one row whose effect is plain to see.
        for c in "every command, searchable".chars() {
            app.press(Key::char(c));
        }
        app.press(Key::named(KeyCode::Enter));
        assert!(app.state.help.is_none());
        assert!(
            app.state.palette.is_some(),
            "Enter ran the help row, the command palette"
        );
        assert_eq!(
            app.state.document.text().to_string(),
            "one\ntwo\n",
            "and opened no search hit"
        );
        assert!(app.state.results.is_some());
    }

    /// The settings tab answers ctrl-s with a reminder; the menu's and the palette's Save went
    /// round that and wrote the file hidden behind the tab.
    #[test]
    fn save_from_the_menu_over_the_settings_tab_leaves_the_file_behind_it_alone() {
        let dir = scratch("save-over-settings");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        app.press(Key::char('x'));
        assert!(app.state.document.is_dirty());
        app.open_settings_tab();
        assert!(app.state.settings_showing());

        app.run(Action::Save);
        assert_eq!(std::fs::read_to_string(dir.join("a.rs")).unwrap(), "one\n");
        assert!(app.state.document.is_dirty(), "nothing was saved");
    }

    /// Only Enter on a tree row was tested, so the click — the way most people open a file from a
    /// tree, and the path that hands the keys back to the text — had nothing driving it.
    #[test]
    fn clicking_a_tree_row_opens_that_file() {
        use dacode::focus::Focus;
        let dir = scratch("tree-click-open");
        std::fs::write(dir.join("b.rs"), "bee\n").unwrap();
        let mut app = editor(&dir, "a.rs", "one\n");
        indexed(&mut app);
        app.show_tree();

        let sidebar = app.layout().sidebar.expect("the tree is a sidebar view");
        let panel = dacode::tree::panel(sidebar, app.scale(), app.state.tree.as_ref().unwrap());
        let (_, rect) = panel
            .rows
            .iter()
            .find(|(row, _)| row.relative == "b.rs")
            .expect("the file has a row");
        app.pointer = middle(*rect);
        app.click(false);
        assert_eq!(app.state.document.text().to_string(), "bee\n");
        assert!(
            app.state
                .file_path
                .as_deref()
                .is_some_and(|path| path.ends_with("b.rs")),
            "the clicked file is the one in front"
        );
        assert_eq!(app.state.focus, Focus::Editor, "and it has the keys");
    }

    /// `welcome::geometry` and `click_welcome` had no test at all, on the one screen where clicking
    /// is the answer to the question being asked.
    #[test]
    fn clicking_the_wizards_cards_answers_and_moves_on() {
        use dacode::welcome::{Step, Welcome};
        let dir = scratch("welcome-click");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.welcome = Some(Welcome::new());
        let option = |app: &App, index: usize| {
            let welcome = app.state.welcome.as_ref().expect("still up");
            middle(
                dacode::welcome::geometry(
                    welcome,
                    app.window_bounds(),
                    app.scale(),
                    &app.state.typography,
                )
                .options[index],
            )
        };

        app.pointer = option(&app, 1);
        app.click(false);
        let welcome = app.state.welcome.as_ref().unwrap();
        assert_eq!(welcome.step, Step::Theme, "choosing a card advances");
        assert!(welcome.answers.modal, "the second card is Vim's keys");

        app.pointer = option(&app, 2);
        app.click(false);
        let welcome = app.state.welcome.as_ref().unwrap();
        assert_eq!(welcome.step, Step::Import);
        assert_eq!(welcome.answers.theme, dacode_theme::Theme::grouped()[2]);
    }

    /// A context-menu row has to run its command, through the real right-click and the real click.
    /// The bar's menus were covered through the same `click_menu`; the context menu's own rows were
    /// not, and they are what a right-click is for.
    #[test]
    fn clicking_a_context_menu_row_runs_its_command() {
        let dir = scratch("context-menu-click");
        let mut app = editor(&dir, "a.rs", "one\ntwo\n");
        let editor_area = app.layout().editor;
        app.pointer = (editor_area.x + 200.0, editor_area.y + 200.0);
        app.open_context_menu();
        let menu = app.state.menu.as_ref().expect("a right-click opens it");
        assert_eq!(menu.kind, dacode::menu::Kind::Context);
        let geometry = menu.geometry(app.window_bounds(), app.scale(), &app.state.typography);
        let wanted = Action::from_name("select_all").unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| item.action == wanted)
            .expect("select all is in the context menu");

        app.pointer = middle(geometry.rows[index]);
        app.click(false);
        assert!(app.state.menu.is_none());
        let primary = app.state.document.selections().primary();
        assert_eq!(
            (primary.start().0, primary.end().0),
            (0, app.state.document.text().len_chars().0),
            "the whole file is selected"
        );
    }

    fn middle(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    /// At 640x480 the results panel has room for fewer rows than its window used to hold, and
    /// stepping down moved the selection onto rows cut off below the panel. Every step, by key
    /// and by wheel, must leave the selection on a row that is drawn — and a click on a drawn row
    /// must open that row.
    #[test]
    fn at_640_pixels_the_results_selection_is_always_on_a_drawn_row() {
        let dir = scratch("results-640");
        let text: String = (0..50).map(|line| format!("needle {line}\n")).collect();
        let mut app = editor(&dir, "a.rs", &text);
        app.headless_size = (640, 480);
        let hits = (0..40)
            .map(|line| dacode_project::SearchHit {
                path: dir.join("a.rs"),
                line_number: line + 1,
                line: format!("needle {line}"),
                range: 0..6,
            })
            .collect();
        app.state
            .show_results(dacode::results::Results::from_hits(&dir, "needle", hits));

        let drawn = |app: &App| {
            let panel = app.layout().results.expect("the panel is open");
            let results = app.state.results.as_ref().unwrap();
            let geometry = results.geometry(panel, app.scale());
            assert!(
                geometry
                    .rows
                    .iter()
                    .all(|row| row.bottom() <= panel.bottom() + 0.01)
            );
            (geometry, results.selected)
        };
        let (geometry, _) = drawn(&app);
        assert!(
            geometry.rows.len() < dacode::results::VISIBLE_ROWS,
            "the window is short enough here to test anything"
        );
        for step in 0..15 {
            if step % 2 == 0 {
                app.press(Key::named(KeyCode::Down));
            } else {
                app.pointer = middle(app.layout().results.unwrap());
                app.wheel(1);
            }
            let (geometry, selected) = drawn(&app);
            assert!(
                (geometry.first..geometry.first + geometry.rows.len()).contains(&selected),
                "after step {step}, hit {selected} is selected and rows {}..{} are drawn",
                geometry.first,
                geometry.first + geometry.rows.len()
            );
        }

        let (geometry, _) = drawn(&app);
        app.pointer = middle(geometry.rows[1]);
        app.click(false);
        assert_eq!(
            app.state.cursor_point().line,
            geometry.first + 1,
            "the click opened the hit drawn under it"
        );
    }

    /// The gap `CLAUDE.md` listed: the palette's rows could only be reached by the keyboard. A
    /// click on row `n` must run row `n` — not its neighbour, and not the one that is selected.
    #[test]
    fn clicking_a_palette_row_runs_that_item() {
        let dir = scratch("palette-click");
        let mut app = editor(&dir, "a.rs", "one\n");
        for name in ["b.rs", "c.rs", "d.rs"] {
            std::fs::write(dir.join(name), "x\n").unwrap();
            app.activate(Target::File(dir.join(name)));
        }
        app.open_palette(PaletteKind::Buffers);
        let geometry = app.palette_geometry().expect("open");
        let palette = app.state.palette.as_ref().unwrap();
        assert!(geometry.rows.len() >= 3);
        let n = 2;
        assert_ne!(
            geometry.items[n], palette.selected,
            "not just Enter on the selection"
        );
        let Target::Buffer(wanted) = palette.rows[geometry.items[n]].target.clone() else {
            panic!("a buffer palette lists buffers");
        };

        app.pointer = middle(geometry.rows[n]);
        app.click(false);
        assert!(
            app.state.palette.is_none(),
            "choosing closes it, as Enter does"
        );
        assert_eq!(app.state.active_buffer, wanted);
    }

    #[test]
    fn clicking_a_help_row_runs_its_command() {
        let dir = scratch("help-click");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.help = Some(dacode::help::Help::new(app.state.modal));
        for c in "every command".chars() {
            app.press(Key::char(c));
        }
        let geometry = app.help_geometry().expect("open");
        let help = app.state.help.as_ref().unwrap();
        let row = (0..geometry.rows.len())
            .find(|at| help.command_on(geometry.first + at) == Some("command_palette"))
            .expect("the filter left the palette's row");

        app.pointer = middle(geometry.rows[row]);
        app.click(false);
        assert!(app.state.help.is_none());
        assert_eq!(
            app.state.palette.as_ref().map(|palette| palette.kind),
            Some(PaletteKind::Commands),
            "the row ran the command palette"
        );
    }

    /// Enter on the help panel runs the highlighted key's command — what makes the panel a way to
    /// do something rather than a page to read.
    #[test]
    fn enter_on_the_help_panel_runs_the_highlighted_command() {
        let dir = scratch("help-enter");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.help = Some(dacode::help::Help::new(app.state.modal));
        for c in "every command".chars() {
            app.press(Key::char(c));
        }
        app.press(Key::named(KeyCode::Enter));
        assert!(app.state.help.is_none());
        assert_eq!(
            app.state.palette.as_ref().map(|palette| palette.kind),
            Some(PaletteKind::Commands)
        );
    }

    /// The second press of a double-click on a palette row lands on the file the first one opened.
    /// It must select nothing there and leave the keys where they were.
    #[test]
    fn a_double_click_on_a_palette_row_does_not_reach_the_text_behind_it() {
        let dir = scratch("palette-double");
        let mut app = editor(&dir, "a.rs", "one\n");
        std::fs::write(dir.join("b.rs"), "alpha beta gamma\n").unwrap();
        app.activate(Target::File(dir.join("b.rs")));
        app.state.tree = Some(dacode::tree::FileTree::new(dir.clone(), &[]));
        app.state.focus = dacode::focus::Focus::Sidebar;
        app.open_palette(PaletteKind::Buffers);
        let geometry = app.palette_geometry().unwrap();
        let row = geometry
            .items
            .iter()
            .position(|item| app.state.palette.as_ref().unwrap().rows[*item].name == "a.rs")
            .expect("a.rs is listed");
        app.pointer = middle(geometry.rows[row]);
        app.click_count = 1;
        app.click(false);
        assert!(app.state.palette.is_none());
        let before = app.state.document.selections().clone();

        // The second press of the series, at the same place, and a jitter of the held button.
        app.click_count = 2;
        app.click(false);
        app.click(true);
        assert_eq!(
            app.state.document.selections(),
            &before,
            "no word was selected"
        );
        assert_eq!(app.state.focus, dacode::focus::Focus::Sidebar);

        // A fresh press is an ordinary click again.
        app.click_count = 1;
        app.click(false);
        assert_eq!(app.state.focus, dacode::focus::Focus::Editor);
    }

    /// A click outside the card closes the palette — including just under it, where its shadow is:
    /// the shadow is drawing, not a target. A click on the query field leaves it open.
    #[test]
    fn the_palette_closes_on_a_click_away_and_not_on_its_input() {
        let dir = scratch("palette-away");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.open_palette(PaletteKind::Commands);
        let geometry = app.palette_geometry().unwrap();
        let before = app.state.document.text().to_string();

        app.pointer = middle(geometry.input);
        app.click(false);
        assert!(app.state.palette.is_some(), "the input is part of the card");

        app.pointer = (middle(geometry.card).0, geometry.card.bottom() + 3.0);
        app.click(false);
        assert!(app.state.palette.is_none(), "the shadow takes no clicks");
        assert_eq!(app.state.document.text().to_string(), before);
    }

    /// A query too long for the field gives way to "N of M" rather than being drawn under it.
    #[test]
    fn a_long_query_does_not_run_under_the_match_count() {
        let dir = scratch("palette-count");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.headless_size = (640, 480);
        app.open_palette(PaletteKind::Commands);
        for c in "a query typed far longer than the field has any room to show".chars() {
            app.press(Key::char(c));
        }
        let mut frame = Frame::default();
        let bounds = app.window_bounds();
        chrome::draw(&mut frame, &app.state, bounds, app.scale());
        let runs: Vec<_> = frame.texts_on(dacode_render::Layer::Modal).collect();
        let count = runs
            .iter()
            .find(|run| run.text.contains(" of "))
            .expect("the count is drawn");
        let query = runs
            .iter()
            .find(|run| run.text.ends_with("room to show"))
            .expect("the query is drawn, losing its start");
        assert!(
            query.rect.right() <= count.rect.x + 0.5,
            "the query's box {:?} runs under the count's {:?}",
            query.rect,
            count.rect
        );
    }

    /// The command centre opened the palette, so a click on it while the palette is open must not
    /// replace the palette with a new one and lose what was typed.
    #[test]
    fn a_click_on_the_command_centre_keeps_the_open_palette_and_its_query() {
        let dir = scratch("palette-anchor");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.open_palette(PaletteKind::Files);
        app.press(Key::char('a'));
        let anchor =
            dacode::palette::anchor(app.window_bounds(), app.scale(), &app.state.typography)
                .expect("the command centre is on the bar at this size");
        app.pointer = middle(anchor);
        app.click(false);
        let palette = app.state.palette.as_ref().expect("still open");
        assert_eq!(palette.kind, PaletteKind::Files);
        assert_eq!(palette.query, "a");
    }

    #[test]
    fn the_help_panel_closes_on_a_click_outside_its_card() {
        let dir = scratch("help-away");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.help = Some(dacode::help::Help::new(app.state.modal));
        let geometry = app.help_geometry().unwrap();
        app.pointer = middle(geometry.input);
        app.click(false);
        assert!(app.state.help.is_some());
        app.pointer = (geometry.card.x / 2.0, middle(geometry.card).1);
        app.click(false);
        assert!(app.state.help.is_none());
    }

    /// Choosing with the mouse leaves the keys where Enter would: with the tree focused, a row
    /// clicked over the code area must not hand the keys to the editor under the card.
    #[test]
    fn a_row_chosen_by_mouse_leaves_focus_where_it_was() {
        let dir = scratch("palette-focus");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.tree = Some(dacode::tree::FileTree::new(dir.clone(), &[]));
        app.state.focus = dacode::focus::Focus::Sidebar;
        app.open_palette(PaletteKind::Commands);
        // Typed, so the row is on screen whatever the table's order puts on the first page.
        for c in "wrap long".chars() {
            app.press(Key::char(c));
        }
        let geometry = app.palette_geometry().unwrap();
        let row = geometry
            .items
            .iter()
            .position(|item| {
                app.state.palette.as_ref().unwrap().rows[*item].target
                    == Target::Command("toggle_soft_wrap")
            })
            .expect("the soft-wrap row is on screen");
        app.pointer = middle(geometry.rows[row]);
        app.click(false);
        assert!(app.state.palette.is_none());
        assert_eq!(app.state.focus, dacode::focus::Focus::Sidebar);
    }

    /// The wheel over the help panel moves it a line at a time and stops at its own last page —
    /// the panel's, measured by its geometry, not the code area's.
    #[test]
    fn the_wheel_scrolls_the_help_panel_to_its_last_page() {
        let dir = scratch("help-wheel");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.state.help = Some(dacode::help::Help::new(app.state.modal));
        app.wheel(1000);
        let geometry = app.help_geometry().unwrap();
        let total = app.state.help.as_ref().unwrap().lines().len();
        assert_eq!(
            geometry.first + geometry.rows.len(),
            total,
            "the last line is on screen"
        );
        assert_eq!(
            geometry.rows.len(),
            geometry.capacity,
            "and the page is full"
        );
        let on_page = |app: &App| {
            let geometry = app.help_geometry().unwrap();
            let help = app.state.help.as_ref().unwrap();
            (geometry.first..geometry.first + geometry.rows.len()).contains(&help.selected)
                && help.selection().is_some()
        };
        assert!(
            on_page(&app),
            "the highlight came onto the page the wheel scrolled to"
        );
        app.press(Key::named(KeyCode::PageUp));
        assert!(on_page(&app), "and onto the one PageUp scrolled to");
    }

    /// A title clicked with the palette open used to open a menu the keyboard could not drive —
    /// the palette took every key, and Escape closed it rather than the menu on top.
    #[test]
    fn a_title_clicked_over_the_palette_closes_it_and_takes_the_keys() {
        let dir = scratch("menu-over-palette");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.open_palette(PaletteKind::Commands);
        assert!(app.state.palette.is_some());
        let layout = app.layout();
        let bar = dacode::layout::title_bar(layout.title, app.scale(), &app.state.typography);
        app.pointer = (bar.menus[1].x + 4.0, bar.menus[1].y + 4.0);
        app.click(false);

        assert!(app.state.palette.is_none(), "the menu replaced the palette");
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.open), Some(1));
        app.press(Key::named(KeyCode::Down));
        assert_eq!(app.state.menu.as_ref().map(|menu| menu.selected), Some(1));
        app.press(Key::named(KeyCode::Escape));
        assert!(app.state.menu.is_none());
    }

    /// At 640 pixels five menus are behind "…". Help, the last, is reached by the mouse alone:
    /// "…" lists the hidden titles, Help's row opens Help under it, and its first row runs.
    #[test]
    fn at_640_pixels_the_mouse_reaches_help_through_the_overflow() {
        let dir = scratch("menu-overflow");
        let mut app = editor(&dir, "a.rs", "one\n");
        app.headless_size = (640, 480);
        let middle = |rect: Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let layout = app.layout();
        let bar = dacode::layout::title_bar(layout.title, app.scale(), &app.state.typography);
        let overflow = bar.overflow.expect("640px hides some menus");
        assert!(
            !bar.menus.is_empty() && bar.menus.len() < dacode::menu::TITLES.len(),
            "Help is behind the overflow at this size"
        );

        app.pointer = middle(overflow);
        app.click(false);
        let menu = app
            .state
            .menu
            .as_ref()
            .expect("the overflow opened its list");
        assert!(menu.listing);
        let geometry = menu.geometry(app.window_bounds(), app.scale(), &app.state.typography);
        let row = menu
            .items
            .iter()
            .position(|item| item.label == "Help")
            .expect("Help is listed");
        app.pointer = middle(geometry.rows[row]);
        app.state.pointer = app.pointer;
        app.move_over_menu();
        app.click(false);

        let menu = app.state.menu.as_ref().expect("Help is open");
        assert_eq!(dacode::menu::TITLES[menu.open], "Help");
        assert!(!menu.listing);
        let geometry = menu.geometry(app.window_bounds(), app.scale(), &app.state.typography);
        assert!(geometry.header.is_some(), "the card names the hidden menu");
        app.pointer = middle(geometry.rows[0]);
        app.click(false);
        assert!(app.state.menu.is_none());
        assert!(
            app.state.help.is_some(),
            "Help's first row opened the help panel"
        );
    }

    /// A right-click menu has no titles, but the bar's are beside it: one click on File opens
    /// File, rather than closing the context menu and waiting for a second click.
    #[test]
    fn a_title_clicked_over_a_context_menu_opens_in_one_click() {
        let dir = scratch("menu-context-title");
        let mut app = editor(&dir, "a.rs", "one\n");
        let layout = app.layout();
        app.pointer = (layout.editor.x + 200.0, layout.editor.y + 200.0);
        app.open_context_menu();
        assert_eq!(
            app.state.menu.as_ref().map(|menu| menu.kind),
            Some(dacode::menu::Kind::Context)
        );
        let bar = dacode::layout::title_bar(layout.title, app.scale(), &app.state.typography);
        app.pointer = (bar.menus[0].x + 4.0, bar.menus[0].y + 4.0);
        app.click(false);
        let menu = app.state.menu.as_ref().expect("File is open");
        assert_eq!(menu.kind, dacode::menu::Kind::Bar);
        assert_eq!(menu.open, 0);
    }

    /// Escape in the tree gives the keys back and leaves the tree standing: it is a view you were
    /// reading, and closing it threw away the place you had reached just to type.
    #[test]
    fn escape_in_the_tree_returns_the_keys_to_the_text() {
        use dacode::focus::Focus;
        let dir = scratch("focus-escape");
        let mut app = editor(&dir, "a.rs", "one\ntwo\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        app.show_tree();
        assert_eq!(
            app.state.focus,
            Focus::Sidebar,
            "opening a view gives it the keys"
        );

        app.press(Key::named(KeyCode::Escape));
        assert_eq!(app.state.focus, Focus::Editor);
        assert!(
            app.state.tree.is_some(),
            "the tree is still there to look at"
        );
    }

    /// The two focus commands are the keyboard's way across, and the second must not close the tree
    /// the way `ctrl-e` does — that is the whole reason it is a separate action.
    #[test]
    fn the_focus_keys_cross_between_the_tree_and_the_text() {
        use dacode::focus::Focus;
        let dir = scratch("focus-keys");
        let mut app = editor(&dir, "a.rs", "one\ntwo\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        assert!(app.state.tree.is_none());

        // Closed tree: asking for it opens it, and gives it the keys.
        app.press(Key::ctrl('e').with_shift());
        assert!(app.state.tree.is_some());
        assert_eq!(app.state.focus, Focus::Sidebar);

        app.press(Key::ctrl('1'));
        assert_eq!(app.state.focus, Focus::Editor);
        assert!(
            app.state.tree.is_some(),
            "focusing the text does not close the tree"
        );

        // Open tree: asking again returns to it rather than toggling it shut.
        app.press(Key::ctrl('e').with_shift());
        assert!(app.state.tree.is_some());
        assert_eq!(app.state.focus, Focus::Sidebar);
    }

    /// Clicking is what moves focus: the tree's rectangle and the text's are the same ones the
    /// drawing reads, so a click where it looks like the text is the text.
    #[test]
    fn clicking_the_text_takes_the_keys_back_from_the_tree() {
        use dacode::focus::Focus;
        let dir = scratch("focus-click");
        let mut app = editor(&dir, "a.rs", "one\ntwo\n");
        app.state.modal = false;
        app.grammar.set_modal(false);
        app.show_tree();
        assert_eq!(app.state.focus, Focus::Sidebar);

        let layout = app.layout();
        app.pointer = (layout.editor.x + 200.0, layout.editor.y + 60.0);
        app.click(false);
        assert_eq!(app.state.focus, Focus::Editor);

        let sidebar = layout.sidebar.expect("the tree is open");
        app.pointer = (sidebar.x + 20.0, sidebar.y + 60.0);
        app.click(false);
        assert_eq!(app.state.focus, Focus::Sidebar);
    }

    /// An editor as the real one is built: with a waker, which is what makes the loop sleep.
    fn woken_editor(dir: &std::path::Path, name: &str, contents: &str) -> App {
        let mut app = editor(dir, name, contents);
        app.waker = Some(Arc::new(|| {}));
        app
    }

    /// The window every chrome test here draws into.
    const WINDOW: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1400.0,
        height: 900.0,
    };

    fn ms(start: std::time::Instant, ms: u64) -> std::time::Instant {
        start + std::time::Duration::from_millis(ms)
    }

    /// Draws the editor as it is at `now`, the way a frame does.
    fn draw_at(app: &mut App, now: std::time::Instant) -> Frame {
        app.state.chrome.now = Some(now);
        let mut frame = Frame::default();
        chrome::draw(&mut frame, &app.state, WINDOW, 1.0);
        frame
    }

    // ---- the settings tab ---------------------------------------------------------------------

    /// An editor with the settings tab open on `category`, and the window the chrome tests draw.
    fn with_settings(dir: &std::path::Path, category: dacode::settings_ui::Category) -> App {
        let mut app = editor(dir, "a.rs", "fn main() {}\n");
        app.headless_size = (WINDOW.width as u32, WINDOW.height as u32);
        let _ = app.run(Action::ToggleSettings);
        let modal = app.state.modal;
        if let Some(page) = &mut app.state.settings_page {
            page.set_category(category, modal);
        }
        app.state.focus = dacode::focus::Focus::Editor;
        app
    }

    /// Down to Accessibility › Size › Font size, then three steps up, by the keys.
    fn three_font_size_steps(app: &mut App) -> Vec<std::time::Instant> {
        app.press(Key::named(KeyCode::Down));
        assert_eq!(
            app.state
                .settings_page
                .as_ref()
                .and_then(|page| page.selected_entry(app.state.modal)),
            Some(dacode::settings_ui::Entry::Field(
                dacode::settings_ui::Field::FontSize
            )),
            "↓ from the theme is the font size"
        );
        (0..3)
            .map(|_| {
                app.press(Key::named(KeyCode::Right));
                app.settings_changed_at
                    .expect("a change is waiting to be written")
            })
            .collect()
    }

    #[test]
    fn three_quick_changes_write_init_lua_once_after_the_last() {
        // There is no save button and no ctrl-s: a change applies at once and is written after a
        // pause. Three steps of the font size are one write of the final value — not three, and not
        // until the pause has run from the *last* of them, so clicking on is never interrupted by a
        // write of a value about to be replaced.
        let dir = scratch("settings-autosave");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        let init = app.config_dir.join("init.lua");
        let size = app.state.typography.code_size;

        let changes = three_font_size_steps(&mut app);
        let (first, last) = (changes[0], changes[2]);
        assert!(last > first, "each change moves the deadline");
        assert_eq!(
            app.state.typography.code_size,
            size + 3.0,
            "applied at once"
        );

        assert!(
            !app.save_settings_if_due(first + SETTINGS_SAVE_AFTER),
            "the pause runs from the last change, not the first"
        );
        assert!(!app.save_settings_if_due(
            last + SETTINGS_SAVE_AFTER - std::time::Duration::from_millis(1)
        ));
        assert!(!init.exists(), "nothing written before the pause is over");

        assert!(app.save_settings_if_due(last + SETTINGS_SAVE_AFTER));
        let written = std::fs::read_to_string(&init).expect("written once the pause is over");
        assert!(
            !app.save_settings_if_due(last + std::time::Duration::from_secs(10)),
            "and once: nothing is left waiting"
        );

        // Through `write_back`'s block, and a program the real config reader reads back the same.
        let config = dacode_lua::eval(&written, "init.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.font_size, Some(size + 3.0));
        assert_eq!(written.matches("dacode.set").count(), 1, "{written}");
    }

    // ---- the Keys page -----------------------------------------------------------------------

    /// The Keys page with Vim off, which is the grammar whose keys the page's tests name. Set on
    /// both halves here rather than read from the machine's config, which `new_state` would.
    fn keys_page(label: &str) -> (std::path::PathBuf, App) {
        let dir = scratch(label);
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Keys);
        app.grammar.set_modal(false);
        app.state.modal = false;
        app.state.mode = Mode::Insert;
        dacode_input::shortcuts::install(dacode_input::Shortcuts::new());
        (dir, app)
    }

    /// A command's row as the mouse sees it: selected, scrolled onto the screen, and laid out by
    /// the geometry the drawing reads.
    fn key_row(app: &mut App, name: &str) -> dacode::settings_ui::PageRow {
        use dacode::settings_ui::Entry;
        let modal = app.state.modal;
        let (rect, scale, typography) = (app.layout().editor, app.scale(), app.state.typography);
        let page = app.state.settings_page.as_mut().expect("the tab is open");
        let index = page
            .entries(modal)
            .iter()
            .position(|entry| matches!(entry, Entry::Command(command) if command.name == name))
            .expect("the command is on the page");
        page.select(index, modal);
        page.reveal(rect, scale, modal, &typography);
        app.state.snap_settings_scroll();
        app.settings_page_geometry()
            .expect("the page is showing")
            .rows
            .into_iter()
            .find(|row| row.index == index)
            .expect("revealed, so on screen")
    }

    fn chord(text: &str) -> Key {
        Key::parse_sequence(text)[0]
    }

    fn keys_of(name: &str) -> String {
        dacode_input::commands::by_name(name)
            .unwrap()
            .keys_for(false)
    }

    fn waiting(app: &App) -> bool {
        app.state
            .settings_page
            .as_ref()
            .is_some_and(|page| page.capture().is_some())
    }

    #[test]
    fn a_key_captured_on_the_keys_page_runs_its_command_through_the_real_key_path() {
        // The whole loop, by the mouse and the keyboard: click the row, press the chord, go back to
        // the code, press it — and the old key no longer does it.
        let (_dir, mut app) = keys_page("keys-capture");
        let row = key_row(&mut app, "toggle_comment");
        app.pointer = middle(row.label);
        app.click(false);
        assert!(waiting(&app), "a click on a command row waits for its key");

        app.press(Key {
            code: KeyCode::Char('m'),
            ctrl: true,
            alt: true,
            shift: false,
        });
        assert!(!waiting(&app), "a shortcut ends the wait");
        assert_eq!(keys_of("toggle_comment"), "<c-a-m>");

        app.hide_settings_tab();
        let before = app.state.document.text().to_string();
        app.press(chord("<c-/>"));
        assert_eq!(
            app.state.document.text().to_string(),
            before,
            "the key it was moved off no longer runs it"
        );
        app.press(chord("<c-a-m>"));
        assert!(
            app.state.document.text().to_string().starts_with("//"),
            "{}",
            app.state.document.text()
        );
    }

    #[test]
    fn a_key_taken_from_another_command_asks_first_and_runs_nothing_while_it_waits() {
        // `ctrl-w` closes the tab the page is on. Pressed while a row waits, it must do neither —
        // close the tab or move the key — until it is pressed again.
        let (_dir, mut app) = keys_page("keys-conflict");
        key_row(&mut app, "toggle_comment");
        app.press(Key::named(KeyCode::Enter));
        assert!(
            waiting(&app),
            "Enter on a command row waits for its key, as a click does"
        );

        app.press(chord("<c-w>"));
        assert!(app.state.settings_page.is_some(), "the tab is still open");
        assert!(waiting(&app), "still waiting");
        let text = app
            .state
            .settings_page
            .as_ref()
            .and_then(|page| page.capture())
            .map(|capture| capture.text(false))
            .unwrap();
        assert!(text.contains("close this file"), "{text}");
        assert_eq!(
            keys_of("close_buffer"),
            "<c-w>  <c-f4>",
            "nothing moved yet"
        );

        app.press(chord("<c-w>"));
        assert!(!waiting(&app));
        assert_eq!(keys_of("toggle_comment"), "<c-w>");
        assert_eq!(keys_of("close_buffer"), "<c-f4>");

        // And Escape leaves a row as it was.
        key_row(&mut app, "save");
        app.press(Key::named(KeyCode::Enter));
        app.press(Key::named(KeyCode::Escape));
        assert!(!waiting(&app));
        assert!(
            app.state.settings_page.is_some(),
            "Escape ended the wait, not the tab"
        );
        assert_eq!(keys_of("save"), "<c-s>");
    }

    #[test]
    fn every_surface_that_prints_a_key_prints_the_moved_one() {
        // A key moved on the page and still printed in the menus or the palette under its old
        // command teaches a key that now does something else.
        let (_dir, mut app) = keys_page("keys-shown");
        key_row(&mut app, "toggle_comment");
        app.press(Key::named(KeyCode::Enter));
        app.press(chord("<c-w>"));
        app.press(chord("<c-w>"));

        let file = dacode::menu::Menu::bar_at(0, false);
        let close = file
            .items
            .iter()
            .find(|item| item.label == "close this file")
            .expect("in the File menu");
        assert_eq!(close.keys, "Ctrl+F4");

        app.open_palette(PaletteKind::Commands);
        let palette = app.state.palette.as_ref().expect("open");
        let row = |what: &str| {
            palette
                .rows
                .iter()
                .find(|row| row.name == what)
                .map(|row| row.context.clone())
        };
        assert_eq!(row("close this file").as_deref(), Some("Ctrl+F4"));
        assert_eq!(
            row("comment or uncomment these lines").as_deref(),
            Some("Ctrl+W")
        );

        let help = dacode::help::Help::new(false);
        assert!(help.lines().contains(&dacode::help::Line::Binding {
            name: "toggle_comment",
            keys: "<c-w>".to_string(),
            what: "comment or uncomment these lines",
        }));
    }

    #[test]
    fn backspace_leaves_a_command_with_no_key_and_the_reset_puts_it_back() {
        let (_dir, mut app) = keys_page("keys-reset");
        key_row(&mut app, "close_buffer");
        app.press(Key::named(KeyCode::Enter));
        app.press(Key::named(KeyCode::Backspace));
        assert_eq!(keys_of("close_buffer"), "");
        app.press(chord("<c-w>"));
        assert!(
            app.state.settings_page.is_some(),
            "ctrl-w no longer closes the tab either"
        );

        // The ⟲ is drawn only on a row whose keys differ, and a click on it lands on it.
        let row = key_row(&mut app, "close_buffer");
        let reset = row.reset.expect("a moved row offers its reset");
        app.pointer = middle(reset);
        app.click(false);
        assert_eq!(keys_of("close_buffer"), "<c-w>  <c-f4>");
        assert!(
            !waiting(&app),
            "the ⟲ puts the keys back without waiting for one"
        );
        assert!(key_row(&mut app, "close_buffer").reset.is_none());

        // "Reset every key" takes the whole set back.
        key_row(&mut app, "save");
        app.press(Key::named(KeyCode::Enter));
        app.press(chord("<f5>"));
        assert_eq!(keys_of("save"), "<f5>");
        let modal = app.state.modal;
        let page = app.state.settings_page.as_mut().unwrap();
        let link = page
            .entries(modal)
            .iter()
            .position(|entry| {
                *entry == dacode::settings_ui::Entry::Link(dacode::settings_ui::Link::ResetKeys)
            })
            .unwrap();
        page.select(link, modal);
        app.press(Key::named(KeyCode::Enter));
        assert!(dacode_input::shortcuts::current().is_empty());
        assert_eq!(keys_of("save"), "<c-s>");
    }

    #[test]
    fn a_search_on_the_keys_page_finds_a_command_by_its_key_however_it_is_written() {
        let (_dir, mut app) = keys_page("keys-search");
        let modal = app.state.modal;
        for query in ["ctrl-w", "Ctrl+W", "<c-w>"] {
            let page = app.state.settings_page.as_mut().unwrap();
            page.set_query(query, modal);
            assert!(
                page.entries(modal).iter().any(|entry| matches!(
                    entry,
                    dacode::settings_ui::Entry::Command(command) if command.name == "close_buffer"
                )),
                "{query}"
            );
        }
    }

    #[test]
    fn a_moved_key_is_written_to_init_lua_as_dacode_keys_and_read_back_on_start() {
        // `init.lua` stays the one place keys are kept: what the page wrote is read by the reader
        // every start goes through, and the lines a person wrote around it are left alone.
        let (dir, mut app) = keys_page("keys-persist");
        let init = app.config_dir.join("init.lua");
        std::fs::create_dir_all(&app.config_dir).unwrap();
        std::fs::write(
            &init,
            "-- mine\ndacode.map(\"normal\", \"<space>w\", \"save\")\n",
        )
        .unwrap();

        key_row(&mut app, "toggle_comment");
        app.press(Key::named(KeyCode::Enter));
        app.press(chord("<c-k>"));
        app.press(chord("<c-c>"));
        assert_eq!(keys_of("toggle_comment"), "<c-k><c-c>");
        let changed = app.settings_changed_at.expect("a write is waiting");
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));

        let written = std::fs::read_to_string(&init).unwrap();
        assert!(written.starts_with("-- mine\ndacode.map("), "{written}");
        assert!(
            written.contains("dacode.keys {\n  toggle_comment = \"<c-k><c-c>\",\n}\n"),
            "{written}"
        );
        assert!(
            !written.contains("dacode.set"),
            "a key alone writes no settings block: {written}"
        );

        let read = dacode::config::load_from(&app.config_dir);
        assert!(read.problems.is_empty(), "{:?}", read.problems);
        assert_eq!(
            read.shortcuts.lookup(&Key::parse_sequence("<c-k><c-c>")),
            dacode_input::Match::Exact(Action::ToggleComment)
        );

        // A second start installs it, and the key works with nothing done on the page.
        let mut next = editor(&dir, "b.rs", "fn b() {}\n");
        next.config_dir = app.config_dir.clone();
        drop(app);
        dacode_input::shortcuts::install(read.shortcuts.clone());
        next.grammar.set_modal(false);
        next.state.modal = false;
        next.state.mode = Mode::Insert;
        next.press(chord("<c-k>"));
        next.press(chord("<c-c>"));
        assert!(next.state.document.text().to_string().starts_with("//"));

        // Reset every key, and the block goes rather than staying behind empty.
        next.change_shortcuts(dacode_input::Shortcuts::clear);
        assert!(next.save_settings_if_due(
            std::time::Instant::now() + SETTINGS_SAVE_AFTER + std::time::Duration::from_secs(1)
        ));
        let cleared = std::fs::read_to_string(&init).unwrap();
        assert!(!cleared.contains("dacode.keys"), "{cleared}");
        assert!(cleared.contains("-- mine"), "{cleared}");
    }

    #[test]
    fn a_settings_write_that_fails_says_so_on_the_status_line_and_in_the_bell() {
        // An automatic save has nobody watching it. If it fails silently the change is lost at the
        // next start with nothing to say why — so it says so where every other message goes.
        //
        // A file where the config directory should be makes the write fail for every user on every
        // platform; a read-only directory does not stop root, and CI is sometimes root.
        let dir = scratch("settings-unwritable");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        std::fs::write(dir.join("blocked"), "").unwrap();
        app.config_dir = dir.join("blocked").join("dacode");

        let last = *three_font_size_steps(&mut app).last().unwrap();
        assert!(app.save_settings_if_due(last + SETTINGS_SAVE_AFTER));

        let said = app
            .state
            .hover
            .clone()
            .expect("the failure is on the status line");
        assert!(said.contains("not saved"), "{said}");
        app.record_notice();
        assert!(
            app.state
                .chrome
                .notifications
                .iter()
                .any(|message| message.contains("not saved")),
            "and in the bell: {:?}",
            app.state.chrome.notifications
        );
        assert!(
            app.settings_changed_at.is_none(),
            "tried once, not on every pass of the loop"
        );
    }

    #[test]
    fn a_waiting_settings_change_sets_an_alarm_for_its_write() {
        // The loop sleeps until input. A change made just before the editor goes idle has nothing to
        // wake it for its write, so the write's deadline has to be in the sleep — as autosave's is.
        let dir = scratch("settings-alarm");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        let now = std::time::Instant::now();
        assert_eq!(app.control_flow(now), ControlFlow::Wait);

        app.settings_changed_at = Some(now);
        assert_eq!(
            app.control_flow(now),
            ControlFlow::WaitUntil(now + SETTINGS_SAVE_AFTER)
        );
        assert_eq!(
            app.control_flow(now + SETTINGS_SAVE_AFTER),
            ControlFlow::Wait,
            "a deadline already reached is the idle step's to act on; waiting on it would spin"
        );
    }

    #[test]
    fn leaving_with_a_settings_change_still_waiting_writes_it() {
        // Quitting inside the pause must not drop the change the pause was waiting to write.
        let dir = scratch("settings-quit");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        let size = app.state.typography.code_size;
        three_font_size_steps(&mut app);

        app.write_before_leaving();
        let written = std::fs::read_to_string(app.config_dir.join("init.lua"))
            .expect("written on the way out");
        assert_eq!(
            dacode_lua::eval(&written, "init.lua").font_size,
            Some(size + 3.0)
        );
    }

    #[test]
    fn the_gear_opens_the_settings_tab_and_the_tab_bar_lists_it_with_the_gear() {
        let dir = scratch("settings-gear");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        app.headless_size = (WINDOW.width as u32, WINDOW.height as u32);
        let layout = app.layout();
        let (_, gear) = dacode::layout::activity_items(layout.activity, 1.0)
            .into_iter()
            .find(|(item, _)| item.activity == dacode::layout::Activity::Settings)
            .expect("the gear is in the activity bar");
        app.pointer = middle(gear);
        app.click(false);

        assert!(app.state.settings_showing(), "the gear opens the tab");
        assert!(
            !app.state.sidebar_open(),
            "a tab, not a view of the sidebar"
        );
        let names = app.state.tab_names();
        let bar = app.tab_bar(layout.tabs, 1.0);
        let tab = bar
            .tabs
            .iter()
            .find(|tab| names[tab.index] == "Settings")
            .expect("the tab bar lists Settings");
        assert_eq!(Some(tab.index), app.state.settings_tab());
        assert_eq!(app.state.active_tab(), tab.index, "and it is the one shown");

        // Drawn with the gear: every stroke the gear icon makes in that box is in the frame.
        let frame = draw_at(&mut app, std::time::Instant::now());
        let mut gear = Frame::default();
        gear.icon(dacode_render::Icon::Gear, tab.icon, app.state.theme.ink_dim);
        let drawn: Vec<_> = frame
            .shapes_on(dacode_render::Layer::Base)
            .map(|shape| shape.kind)
            .collect();
        let wanted: Vec<_> = gear
            .shapes_on(dacode_render::Layer::Base)
            .map(|shape| shape.kind)
            .collect();
        assert!(!wanted.is_empty());
        assert!(
            wanted.iter().all(|kind| drawn.contains(kind)),
            "the settings tab carries the gear icon"
        );

        // And again: the gear shows the tab, it does not close it.
        app.click(false);
        assert!(app.state.settings_showing());
    }

    #[test]
    fn a_click_on_a_settings_control_changes_the_value_and_lights_the_row() {
        use dacode::settings_ui::Field;
        let dir = scratch("settings-click");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Editor);
        let geometry = app.settings_page_geometry().expect("the page is showing");
        let row = geometry
            .rows
            .iter()
            .find(|row| row.field() == Some(Field::SoftWrap))
            .expect("Editor › Wrapping is on screen");
        let before = app.state.display.mode;

        app.pointer = middle(row.control.expect("a switch"));
        app.click(false);

        assert_ne!(app.state.display.mode, before, "the switch flipped");
        let glow = app.state.chrome.motion.get(
            dacode::motion::Key::Glow(dacode::settings_ui::glow_index(Field::SoftWrap)),
            std::time::Instant::now(),
        );
        assert!(glow > 0.5, "the row it changed glows: {glow}");
        assert!(app.settings_changed_at.is_some(), "and a write is waiting");
        assert_eq!(
            app.state.focus,
            dacode::focus::Focus::Editor,
            "a click on the page gives it the keys"
        );
    }

    fn dropdown_open(app: &App) -> bool {
        app.state
            .settings_page
            .as_ref()
            .is_some_and(|page| page.dropdown().is_some())
    }

    /// The theme dropdown's row for `name`, as the mouse would find it.
    fn dropdown_row(app: &App, name: &str) -> dacode::settings_ui::DropdownRow {
        app.settings_dropdown_geometry()
            .expect("the dropdown is open")
            .rows
            .into_iter()
            .find(|row| row.pick.theme() == Some(name))
            .unwrap_or_else(|| panic!("{name} is in the dropdown"))
    }

    #[test]
    fn a_click_on_a_theme_in_the_dropdown_applies_it_and_saves_it_like_any_change() {
        // Through `App::click` both times, the way a mouse gets there: the control opens the list,
        // and the row picks — live, touched, and waiting to be written, as a switch's click is.
        use dacode::settings_ui::Field;
        let dir = scratch("theme-dropdown-click");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        let start = app.state.theme.name();
        assert_ne!(start, Some("mist-coral"));

        app.pointer = middle(page_row(&app, Field::Theme).control.expect("a control"));
        app.click(false);
        assert!(dropdown_open(&app), "a click on the control opens the list");
        assert_eq!(app.state.theme.name(), start, "opening changes nothing");

        let row = dropdown_row(&app, "mist-coral");
        assert_eq!(row.clip, row.rect, "on screen whole at this size");
        app.pointer = middle(row.clip);
        app.click(false);

        assert_eq!(app.state.theme.name(), Some("mist-coral"), "applied live");
        assert!(!dropdown_open(&app), "choosing closes it");
        assert!(app.settings_touched.contains(&Field::Theme), "touched");
        assert!(app.settings_changed_at.is_some(), "and a write is waiting");
    }

    #[test]
    fn escape_closes_the_theme_dropdown_and_nothing_else() {
        let dir = scratch("theme-dropdown-escape");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        let start = app.state.theme.name();
        // The page opens with the theme row selected; Enter does what a click on it does.
        app.press(Key::named(KeyCode::Enter));
        assert!(dropdown_open(&app), "enter on the row opens it");
        app.press(Key::named(KeyCode::Down));

        app.press(Key::named(KeyCode::Escape));
        assert!(!dropdown_open(&app), "escape closes it");
        assert!(
            app.state.settings_showing(),
            "and only it: the tab is still up"
        );
        assert_eq!(app.state.theme.name(), start, "a highlight is not a choice");
        assert!(app.settings_touched.is_empty());

        // Space opens it too, and a click away from it closes it without choosing.
        app.press(Key::char(' '));
        assert!(dropdown_open(&app), "space on the row opens it");
        let card = app.settings_dropdown_geometry().unwrap().card;
        app.pointer = (card.x - 20.0, card.bottom() - 2.0);
        app.click(false);
        assert!(!dropdown_open(&app), "a click outside closes it");
        assert_eq!(app.state.theme.name(), start);
    }

    #[test]
    fn down_and_enter_in_the_dropdown_pick_the_theme_below() {
        use dacode::settings_ui::Field;
        let dir = scratch("theme-dropdown-keys");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        // The theme a fresh editor starts in is whatever this machine's own `init.lua` says, and
        // when that was the last in the list there was no theme below it to pick. Set here, so
        // the test reads the same on every machine.
        let order = dacode_theme::Theme::grouped();
        let start = order[0];
        app.state.theme = dacode_theme::Theme::by_name(start).expect("a built-in");
        let next = order[1];
        app.press(Key::named(KeyCode::Enter));

        app.press(Key::named(KeyCode::Down));
        assert_eq!(
            app.state.theme.name(),
            Some(start),
            "the arrows move the highlight, not the theme"
        );
        app.press(Key::named(KeyCode::Enter));

        assert_eq!(app.state.theme.name(), Some(next));
        assert!(!dropdown_open(&app));
        assert!(app.settings_touched.contains(&Field::Theme));
        // The page's own selection did not move under the dropdown.
        assert_eq!(
            app.state
                .settings_page
                .as_ref()
                .and_then(|page| page.selected_entry(app.state.modal)),
            Some(dacode::settings_ui::Entry::Field(Field::Theme))
        );
    }

    /// A point on a dropdown row that is also on another setting's control in the page under the
    /// card — where a press that reached the page would change that setting.
    fn dropdown_point_over_another_control(app: &App) -> (f32, f32, dacode::settings_ui::Field) {
        use dacode::settings_ui::Field;
        let page = app.settings_page_geometry().unwrap();
        let dropdown = app.settings_dropdown_geometry().unwrap();
        // Never the theme already in use: picking it changes nothing, and which theme a fresh
        // editor starts in is the machine's light-or-dark preference, so on CI the first row that
        // covered another control was the current theme and the test asserted a change that could
        // not happen.
        let current = app.state.theme.name();
        for row in dropdown
            .rows
            .iter()
            .filter(|row| row.pick.theme().is_some() && row.pick.theme() != current)
        {
            for under in &page.rows {
                let (Some(field), Some(control)) = (under.field(), under.control) else {
                    continue;
                };
                if field == Field::Theme {
                    continue;
                }
                let overlap = row.clip.intersect(control.intersect(under.clip));
                if overlap.width > 2.0 && overlap.height > 2.0 {
                    return (
                        overlap.x + overlap.width / 2.0,
                        overlap.y + overlap.height / 2.0,
                        field,
                    );
                }
            }
        }
        panic!("the card covers no other control at this size");
    }

    #[test]
    fn a_double_click_on_a_dropdown_row_changes_the_theme_and_nothing_under_it() {
        // The first press picks and closes the list; the second, with nothing in front of the page
        // any more, used to land on whatever control the card had been covering — Animations, the
        // font size, transparency — which was then marked touched and written to init.lua.
        use dacode::settings_ui::Field;
        let dir = scratch("theme-dropdown-double");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        app.pointer = middle(page_row(&app, Field::Theme).control.unwrap());
        app.click_count = 1;
        app.click(false);
        assert!(dropdown_open(&app));
        let start = app.state.theme.name();
        let (x, y, under) = dropdown_point_over_another_control(&app);
        let values = app.state.setting_values();

        app.pointer = (x, y);
        app.click_count = 1;
        app.click(false);
        app.click_count = 2;
        app.click(false);
        app.click(true);

        assert_ne!(app.state.theme.name(), start, "the theme was picked");
        assert_eq!(
            app.settings_touched,
            vec![Field::Theme],
            "and only the theme"
        );
        assert!(
            !app.state.setting_values().differs(under, &values),
            "{under:?}, under the card, is as it was"
        );
        // A fresh press is an ordinary click again.
        app.click_count = 1;
        app.click(false);
        assert!(!app.settings_touched.is_empty());
    }

    #[test]
    fn a_double_click_on_the_theme_control_leaves_the_list_open() {
        // The first press opens the list; the second lands on the control again, which is outside
        // the card, and closed it — a list that flashed and was gone.
        use dacode::settings_ui::Field;
        let dir = scratch("theme-dropdown-double-control");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        app.pointer = middle(page_row(&app, Field::Theme).control.unwrap());
        app.click_count = 1;
        app.click(false);
        app.click_count = 2;
        app.click(false);
        app.click(true);
        assert!(dropdown_open(&app), "still open after the second press");
        assert!(app.settings_touched.is_empty());
    }

    #[test]
    fn the_theme_dropdown_does_not_outlive_its_tab_being_hidden() {
        // A file dropped on the window opens it and hides the tab with no key or click reaching the
        // list; coming back to the tab brought the card back with it, taking the next key.
        let dir = scratch("theme-dropdown-hidden");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        std::fs::write(dir.join("dropped.rs"), "fn dropped() {}\n").unwrap();
        app.press(Key::named(KeyCode::Enter));
        assert!(dropdown_open(&app));

        app.activate(Target::File(dir.join("dropped.rs")));
        assert!(!app.state.settings_showing());
        app.open_settings_tab();
        assert!(app.state.settings_showing());
        assert!(!dropdown_open(&app), "the list closed with the tab");
    }

    #[test]
    fn toggling_twice_comes_back_to_the_theme_it_started_from() {
        // True black has no light side; its toggle lands on Linen, whose own other half is Walnut.
        // The toggle remembers where it came from, so a second press goes back.
        let dir = scratch("toggle-back");
        let mut app = editor(&dir, "a.rs", "one\n");
        for start in ["true-black", "slate", "rose-pine", "charcoal-coral"] {
            app.state.theme = dacode_theme::Theme::by_name(start).unwrap();
            let _ = app.run(Action::ToggleTheme);
            assert_ne!(
                app.state.theme.appearance,
                dacode_theme::Theme::by_name(start).unwrap().appearance
            );
            let _ = app.run(Action::ToggleTheme);
            assert_eq!(app.state.theme.name(), Some(start), "back to {start}");
        }
    }

    #[test]
    fn a_row_behind_the_open_theme_list_does_not_light_under_the_pointer() {
        use dacode::settings_ui::Field;
        // The list is drawn after the page, and every wash read the pointer, so the row under the
        // list lit up as the pointer crossed it: two highlights at once, one on a row a click there
        // could never reach. Drawn both ways through the same pointer, so the test can see a wash
        // at all before it asserts there is none.
        let dir = scratch("theme-dropdown-hover");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        app.pointer = middle(page_row(&app, Field::Theme).control.expect("a control"));
        app.click(false);
        let card = app.settings_dropdown_geometry().expect("open").card;
        let behind = app
            .settings_page_geometry()
            .expect("the page is showing")
            .rows
            .into_iter()
            .find(|row| {
                row.field().is_some_and(|field| field != Field::Theme)
                    && row.clip.intersect(card).height > 4.0
            })
            .expect("a setting's row runs under the list");
        let over = behind.clip.intersect(card);
        let point = (over.x + over.width / 2.0, over.y + over.height / 2.0);
        let washed = |frame: &Frame| {
            frame
                .quads_on(dacode_render::Layer::Base)
                .any(|quad| quad.rect == behind.rect)
        };

        let now = std::time::Instant::now();
        app.pointer = point;
        app.state.pointer = point;
        app.sync_motion(now);
        assert!(!washed(&draw_at(&mut app, now)), "covered by the list");

        let _ = app.press(Key::named(KeyCode::Escape));
        app.state.pointer = point;
        app.sync_motion(now);
        assert!(!dropdown_open(&app));
        assert!(
            washed(&draw_at(&mut app, now)),
            "lit once the list is closed"
        );
    }

    #[test]
    fn the_theme_dropdown_fits_a_640_by_480_window_and_scrolls_to_its_last_theme() {
        let dir = scratch("theme-dropdown-small");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Appearance);
        app.headless_size = (640, 480);
        app.press(Key::named(KeyCode::Enter));
        let window = app.window_bounds();
        let editor = app.layout().editor;

        app.press(Key::named(KeyCode::End));
        let geometry = app.settings_dropdown_geometry().expect("open");
        let card = geometry.card;
        assert!(
            card.x >= editor.x
                && card.y >= editor.y
                && card.right() <= editor.right() + 0.01
                && card.bottom() <= editor.bottom() + 0.01,
            "{card:?} inside the editor area of {window:?}"
        );
        assert!(
            geometry.max_scroll > 0.0,
            "fifteen rows do not fit in 480 px"
        );
        let last = dacode_theme::Theme::grouped().last().copied().unwrap();
        let row = dropdown_row(&app, last);
        assert_eq!(
            row.clip, row.rect,
            "End scrolled the last theme into view whole"
        );

        // And the wheel scrolls the list, not the page behind it.
        let page_scroll = app.state.settings_page.as_ref().unwrap().scroll();
        app.pointer = middle(card);
        app.wheel(-3);
        let after = app.settings_dropdown_geometry().unwrap();
        assert!(
            after.scroll < geometry.scroll,
            "{} → {}",
            geometry.scroll,
            after.scroll
        );
        assert_eq!(
            app.state.settings_page.as_ref().unwrap().scroll(),
            page_scroll
        );
    }

    #[test]
    fn down_on_the_settings_tab_moves_the_selection_and_keeps_it_on_screen() {
        // The Keys page is ninety-odd rows, far more than the page shows at this size; walking down
        // it must scroll, or the selection walks off the bottom and the next → changes a row
        // nobody can see.
        let dir = scratch("settings-down");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Keys);
        app.headless_size = (900, 600);
        let caret = app.state.cursor();
        let start = app.state.settings_page.as_ref().unwrap().selected();

        for _ in 0..30 {
            app.press(Key::named(KeyCode::Down));
        }

        let selected = app.state.settings_page.as_ref().unwrap().selected();
        assert!(selected >= start + 30, "{start} → {selected}");
        let geometry = app.settings_page_geometry().unwrap();
        let row = geometry
            .rows
            .iter()
            .find(|row| row.index == selected)
            .expect("the selected row is on screen");
        assert_eq!(row.clip, row.rect, "and whole");
        assert_eq!(
            app.state.cursor(),
            caret,
            "the text behind the tab did not move"
        );
    }

    /// The page's row for `field`, as the mouse would find it.
    fn page_row(app: &App, field: dacode::settings_ui::Field) -> dacode::settings_ui::PageRow {
        app.settings_page_geometry()
            .expect("the page is showing")
            .rows
            .into_iter()
            .find(|row| row.field() == Some(field))
            .unwrap_or_else(|| panic!("{field:?} is on screen"))
    }

    /// Selects `field` on the open page and steps it up once by the keys, as a user would.
    fn step_on_page(app: &mut App, field: dacode::settings_ui::Field) {
        let modal = app.state.modal;
        let page = app.state.settings_page.as_mut().expect("the tab is open");
        let category = dacode::settings_ui::Category::ALL
            .into_iter()
            .find(|category| {
                dacode::settings_ui::sections(*category)
                    .iter()
                    .any(|section| section.fields.contains(&field))
            })
            .unwrap();
        page.set_category(category, modal);
        let index = page
            .entries(modal)
            .iter()
            .position(|entry| *entry == dacode::settings_ui::Entry::Field(field))
            .unwrap();
        page.select(index, modal);
        app.press(Key::named(KeyCode::Right));
    }

    #[test]
    fn a_click_on_a_steppers_minus_lowers_it_and_on_its_plus_raises_it() {
        // The stepper draws two buttons that go opposite ways. They were one control to the
        // mouse, which only cycled upwards, so − raised the font size.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-stepper");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        let size = app.state.typography.code_size;
        let (down, up) = page_row(&app, Field::FontSize)
            .stepper
            .expect("a stepper has two buttons");

        app.pointer = middle(down);
        app.click(false);
        assert_eq!(app.state.typography.code_size, size - 1.0, "− lowers it");

        app.pointer = middle(up);
        app.click(false);
        app.click(false);
        assert_eq!(app.state.typography.code_size, size + 1.0, "+ raises it");
    }

    #[test]
    fn a_right_click_on_the_settings_page_changes_nothing_and_opens_no_menu() {
        // A right-click is the first thing a mouse user tries. It placed the caret by clicking,
        // which flipped the switch under the pointer and wrote it, then opened the code menu
        // over a document nobody could see.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-right-click");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Editor);
        let before = app.state.display.mode;
        app.pointer = middle(page_row(&app, Field::SoftWrap).control.unwrap());
        app.state.pointer = app.pointer;

        app.open_context_menu();

        assert_eq!(app.state.display.mode, before, "the switch did not flip");
        assert!(
            app.settings_changed_at.is_none(),
            "and nothing waits to be written"
        );
        assert!(app.state.menu.is_none(), "and no code menu covers the page");
    }

    #[test]
    fn the_settings_tabs_own_write_does_not_come_back_as_a_config_reload() {
        // Every auto-save came back through the watcher as an outside edit: a reload that walked
        // PATH for the report, reset the keymap and said "config reloaded" after every change.
        let dir = scratch("settings-own-write");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        let sentinel = vec![dacode::settings_ui::ServerRow {
            language: "Sentinel".into(),
            program: "untouched".into(),
            status: "a reload would replace this".into(),
            found: false,
            configured: false,
        }];
        let modal = app.state.modal;
        app.state
            .settings_page
            .as_mut()
            .unwrap()
            .set_reports(sentinel.clone(), Vec::new(), modal);

        let last = *three_font_size_steps(&mut app).last().unwrap();
        assert!(app.save_settings_if_due(last + SETTINGS_SAVE_AFTER));
        let init = app.config_dir.join("init.lua");
        app.state.hover = None;
        app.handle_changed_paths(vec![init.clone()]);
        app.record_notice();

        assert_eq!(app.state.hover, None, "no reload notice");
        assert!(
            app.state.chrome.notifications.is_empty(),
            "nothing in the bell: {:?}",
            app.state.chrome.notifications
        );
        assert_eq!(
            app.state.settings_page.as_ref().unwrap().servers(),
            &sentinel[..],
            "and PATH was not walked again"
        );

        // Someone else's edit to the same file is still a reload.
        std::fs::write(&init, "dacode.set { tab_width = 3 }\n").unwrap();
        app.handle_changed_paths(vec![init]);
        assert_eq!(app.state.hover.as_deref(), Some("config reloaded"));
        assert_eq!(app.state.display.tab_width, 3);
    }

    #[test]
    fn a_session_zoom_is_not_written_when_another_setting_changes() {
        // ctrl-+ for a screen share is for this session. The first write from the page used to pin
        // every live value — the zoom, a toggled theme, what an import brought in — so whether a
        // zoom stuck depended on whether anything else had been touched.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-zoom");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Editor);
        let _ = app.run(Action::IncreaseFontSize);
        let _ = app.run(Action::ToggleTheme);
        step_on_page(&mut app, Field::LineNumbers);

        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));
        let written = std::fs::read_to_string(app.config_dir.join("init.lua")).unwrap();
        let config = dacode_lua::eval(&written, "init.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(
            config.font_size, None,
            "the zoom is not written:\n{written}"
        );
        assert_eq!(config.theme_name, None, "nor the toggled theme");
        assert_eq!(
            config.line_numbers.as_deref(),
            Some(app.state.line_numbers.name()),
            "the setting changed on the page is"
        );
    }

    #[test]
    fn a_viewed_image_opens_as_a_picture_and_saving_never_writes_over_it() {
        // An image used to be refused as binary. Its buffer is empty — the picture is drawn from
        // the file — so a save of that buffer would replace the photo with nothing; ctrl-s and
        // the keys the grammar would have taken must leave the file exactly as it was.
        let dir = scratch("viewer");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 2, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let path = dir.join("photo.png");
        std::fs::write(&path, &png).unwrap();

        app.activate(Target::File(path.clone()));
        assert!(app.state.preview.is_some(), "opened in the viewer");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.state.preview.as_ref().unwrap().status == dacode::viewer::Status::Loading
            && std::time::Instant::now() < deadline
        {
            app.collect_background();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let preview = app.state.preview.as_ref().unwrap();
        assert_eq!(preview.status, dacode::viewer::Status::Ready);
        assert_eq!(
            (preview.pages[0].width, preview.pages[0].height),
            (4.0, 2.0)
        );

        app.press(Key::char('x'));
        app.press(Key::ctrl('s'));
        assert!(
            app.state
                .prompt
                .as_ref()
                .is_some_and(|p| p.message.is_some()),
            "and says why"
        );
        app.press(Key::named(KeyCode::Escape));
        assert_eq!(std::fs::read(&path).unwrap(), png, "the file is untouched");
        assert!(!app.state.document.is_dirty());

        // The keys the text would have had zoom the picture instead.
        app.press(Key::char('+'));
        assert!(app.state.preview.as_ref().unwrap().zoom > 1.0);
        app.press(Key::char('0'));
        assert_eq!(app.state.preview.as_ref().unwrap().zoom, 1.0);

        // And it travels with its buffer: away and back, it is still the picture.
        let _ = app.run(Action::CloseBuffer);
        assert!(app.state.preview.is_none(), "the code file has no viewer");
    }

    #[test]
    fn the_terminal_folds_to_its_header_while_the_settings_page_is_in_front() {
        // The panel kept its height over the settings tab, so the rows being read were under the
        // shell. Folded, the page has the height; giving the terminal the keys — a click on its
        // strip, ctrl-` — brings it back at the height it had.
        let dir = scratch("terminal-fold");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        app.state.terminal.open = true;
        let open = app.layout().terminal.expect("open").height;
        let _ = app.run(Action::ToggleSettings);
        app.state.focus = dacode::focus::Focus::Editor;
        let folded = app.layout().terminal.expect("still there").height;
        assert_eq!(folded, dacode::layout::TERMINAL_HEADER);
        assert!(open > folded * 3.0);

        app.state.focus = dacode::focus::Focus::Terminal;
        assert_eq!(app.layout().terminal.expect("open").height, open);
        app.state.focus = dacode::focus::Focus::Editor;
        let _ = app.run(Action::CloseBuffer);
        assert!(!app.state.settings_showing());
        assert_eq!(
            app.layout().terminal.expect("open").height,
            open,
            "and back over code, at the height it had"
        );
    }

    #[test]
    fn ctrl_equals_zooms_the_whole_interface_in_both_grammars_and_is_kept() {
        // The keys every application zooms with did nothing at all: the only binding was Vim's
        // plain `=`, and it stepped the code font, which stops at 18 px and leaves the menus and
        // panels as they were. Someone who needed everything larger was raising the desktop's
        // scale to get it.
        use dacode::chrome::Zoom;
        use dacode::settings_ui::Field;
        for modal in [true, false] {
            let dir = scratch(&format!("interface-zoom-{modal}"));
            let mut app = editor(&dir, "a.rs", "fn main() {}\n");
            app.grammar.set_modal(modal);
            app.state.modal = modal;
            let before = app.scale();
            app.press(Key::ctrl('='));
            assert!((app.scale() - before * 1.1).abs() < 1e-4, "modal {modal}");
            assert!(
                app.settings_touched.contains(&Field::InterfaceZoom),
                "a zoom someone needs is written, not lost at the next start"
            );
            app.press(Key::ctrl('0'));
            assert_eq!(app.scale(), before);
            for _ in 0..40 {
                app.press(Key::ctrl('-'));
            }
            assert_eq!(app.state.chrome.zoom.percent(), Zoom::MIN, "and it stops");
        }
    }

    #[test]
    fn a_setting_already_in_the_block_keeps_its_value_when_another_is_written() {
        use dacode::settings_ui::Field;
        let dir = scratch("settings-keep-block");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Editor);
        let mut configured = app.state.setting_values();
        configured.font_size = 15.0;
        let init = app.config_dir.join("init.lua");
        std::fs::create_dir_all(&app.config_dir).unwrap();
        std::fs::write(
            &init,
            dacode::settings_ui::write_back("-- mine\n", &configured),
        )
        .unwrap();

        let _ = app.run(Action::IncreaseFontSize);
        step_on_page(&mut app, Field::LineNumbers);
        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));

        let written = std::fs::read_to_string(&init).unwrap();
        let config = dacode_lua::eval(&written, "init.lua");
        assert_eq!(
            config.font_size,
            Some(15.0),
            "kept, not the zoom:\n{written}"
        );
        assert_eq!(
            config.line_numbers.as_deref(),
            Some(app.state.line_numbers.name())
        );
        assert!(
            written.starts_with("-- mine\n"),
            "the user's own lines are kept"
        );
        assert_eq!(written.matches("dacode.set").count(), 1);
    }

    /// An editor with `init.lua` open as a buffer, then the settings tab open over it.
    fn with_init_lua_open(dir: &std::path::Path, contents: &str) -> (App, std::path::PathBuf) {
        let mut app = editor(dir, "a.rs", "fn main() {}\n");
        app.headless_size = (WINDOW.width as u32, WINDOW.height as u32);
        let init = app.config_dir.join("init.lua");
        std::fs::create_dir_all(&app.config_dir).unwrap();
        std::fs::write(&init, contents).unwrap();
        app.activate(Target::File(init.clone()));
        // By identity, not by spelling: Windows' temp directory is an 8.3 short name
        // (`RUNNER~1`) that `canonical` expands, so the raw and resolved paths never compare equal.
        assert_eq!(
            buffer_key(app.state.file_path.as_deref()),
            Some(dacode_project::canonical(&init))
        );
        (app, init)
    }

    #[test]
    fn an_open_clean_init_lua_follows_what_the_settings_tab_writes() {
        // The Keys page's link opens init.lua beside a tab that rewrites it. A clean buffer that
        // kept the old text would, on its next save, write the old block back over the new one.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-init-clean");
        let (mut app, init) = with_init_lua_open(&dir, "-- mine\n");
        let _ = app.run(Action::ToggleSettings);
        app.state.focus = dacode::focus::Focus::Editor;
        step_on_page(&mut app, Field::LineNumbers);
        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));

        let on_disk = std::fs::read_to_string(&init).unwrap();
        assert!(on_disk.contains("line_numbers"));
        assert_eq!(
            app.state.document.text().to_string(),
            on_disk,
            "the buffer shows what was written"
        );
        assert!(!app.state.document.is_dirty());

        // Back on the file, an edit and a save keep the block.
        app.switch_to(app.state.active_buffer);
        app.state.mutate(|document| document.insert("-- more\n"));
        app.run(Action::Save);
        let saved = std::fs::read_to_string(&init).unwrap();
        assert!(saved.contains("-- more"), "{saved}");
        assert!(
            saved.contains("line_numbers"),
            "the block survived the save:\n{saved}"
        );
    }

    #[test]
    fn an_open_init_lua_with_unsaved_edits_holds_the_write_until_it_is_saved() {
        // Writing under a dirty buffer loses one side: its next save overwrites the block, or the
        // block overwrites it. So the write waits, says so, and happens once the buffer is saved.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-init-dirty");
        let (mut app, init) = with_init_lua_open(&dir, "-- mine\n");
        app.state.mutate(|document| document.insert("-- edit\n"));
        assert!(app.state.document.is_dirty());

        let _ = app.run(Action::ToggleSettings);
        app.state.focus = dacode::focus::Focus::Editor;
        step_on_page(&mut app, Field::LineNumbers);
        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));
        assert_eq!(
            std::fs::read_to_string(&init).unwrap(),
            "-- mine\n",
            "not written"
        );
        let said = app.state.hover.clone().unwrap_or_default();
        assert!(said.contains("unsaved edits"), "{said}");
        app.record_notice();
        assert!(
            app.state
                .chrome
                .notifications
                .iter()
                .any(|message| message.contains("unsaved edits"))
        );
        assert!(
            !app.save_settings_if_due(changed + std::time::Duration::from_secs(10)),
            "and not retried while the buffer is still dirty"
        );

        app.switch_to(app.state.active_buffer);
        app.run(Action::Save);
        assert!(
            app.save_settings_if_due(std::time::Instant::now()),
            "saving it lets the write through"
        );
        let on_disk = std::fs::read_to_string(&init).unwrap();
        assert!(
            on_disk.contains("-- edit"),
            "the user's edit is kept:\n{on_disk}"
        );
        assert!(
            on_disk.contains("line_numbers"),
            "and the setting is written:\n{on_disk}"
        );
        assert_eq!(app.state.document.text().to_string(), on_disk);
        assert!(!app.state.document.is_dirty());
    }

    #[test]
    fn a_held_write_keeps_its_change_when_the_save_that_releases_it_is_reported_first() {
        // Saving the open init.lua releases the held write, and the watcher reports that save in
        // the same idle step, ahead of the write. Its reload put the file's old font size back,
        // and the write then pinned the old size where the user had chosen another.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-release-race");
        let (mut app, init) = with_init_lua_open(&dir, "-- mine\n");
        app.state.mutate(|document| document.insert("-- edit\n"));
        let _ = app.run(Action::ToggleSettings);
        app.state.focus = dacode::focus::Focus::Editor;
        let chosen = app.state.typography.code_size + 1.0;
        step_on_page(&mut app, Field::FontSize);
        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));
        assert!(app.settings_blocked, "held for the unsaved buffer");

        app.switch_to(app.state.active_buffer);
        app.run(Action::Save);
        app.handle_changed_paths(vec![init.clone()]);
        assert_eq!(
            app.state.typography.code_size, chosen,
            "the reload kept the change"
        );
        assert!(app.save_settings_if_due(std::time::Instant::now()));

        let written = std::fs::read_to_string(&init).unwrap();
        let config = dacode_lua::eval(&written, "init.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(
            config.font_size,
            Some(chosen),
            "the held change is written:\n{written}"
        );
        assert!(written.contains("-- edit"));
    }

    #[test]
    fn an_outside_edit_inside_the_pause_does_not_undo_a_change_waiting_to_be_written() {
        // Another program — or the import — rewriting init.lua within the 600 ms before a write is
        // read back by the watcher. What it says applies; what the page changed and has not yet
        // written stays, and is then written on top.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-outside-edit");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        let chosen = app.state.typography.code_size + 1.0;
        step_on_page(&mut app, Field::FontSize);
        let changed = app.settings_changed_at.unwrap();

        let init = app.config_dir.join("init.lua");
        std::fs::create_dir_all(&app.config_dir).unwrap();
        std::fs::write(&init, "dacode.set { tab_width = 3 }\n").unwrap();
        app.handle_changed_paths(vec![init.clone()]);
        assert_eq!(app.state.display.tab_width, 3, "the outside edit applies");
        assert_eq!(
            app.state.typography.code_size, chosen,
            "and the change waiting stays"
        );

        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));
        let config = dacode_lua::eval(&std::fs::read_to_string(&init).unwrap(), "init.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.font_size, Some(chosen));
        assert_eq!(config.tab_width, Some(3));
    }

    #[test]
    fn a_hand_edit_to_the_same_setting_made_while_a_change_waits_wins() {
        // The most recent intent applies. A change on the page waits out its pause; meanwhile
        // init.lua is edited by hand to set that very field. The edit is newer: re-applying the
        // page over it reverted the user's 17.0 to 14.5, live and in their file.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-newer-hand-edit");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        step_on_page(&mut app, Field::FontSize);
        let changed = app.settings_changed_at.unwrap();

        let init = app.config_dir.join("init.lua");
        std::fs::create_dir_all(&app.config_dir).unwrap();
        std::fs::write(&init, "dacode.set { font_size = 17.0 }\n").unwrap();
        app.handle_changed_paths(vec![init.clone()]);
        assert_eq!(
            app.state.typography.code_size, 17.0,
            "the newer hand edit wins live"
        );

        app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER);
        let on_disk = std::fs::read_to_string(&init).unwrap();
        assert_eq!(
            dacode_lua::eval(&on_disk, "init.lua").font_size,
            Some(17.0),
            "and on disk:\n{on_disk}"
        );
        assert!(
            !on_disk.contains("14.5"),
            "nothing writes the page's value back:\n{on_disk}"
        );
    }

    #[test]
    fn a_reset_takes_the_setting_out_of_the_block() {
        // Rewritten at the default it would pin today's default over a later import, or over a
        // change to DaCode's own default. Taken out, the file says only what the user chose.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-reset-removes");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Accessibility);
        let mut configured = app.state.setting_values();
        configured.font_size = 15.0;
        let init = app.config_dir.join("init.lua");
        std::fs::create_dir_all(&app.config_dir).unwrap();
        std::fs::write(&init, dacode::settings_ui::write_back("", &configured)).unwrap();
        app.reload_config();
        assert_eq!(app.state.typography.code_size, 15.0);

        let row = page_row(&app, Field::FontSize);
        app.pointer = middle(row.reset.expect("a changed size offers a reset"));
        app.click(false);
        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));

        let written = std::fs::read_to_string(&init).unwrap();
        let config = dacode_lua::eval(&written, "init.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert!(
            !written.contains("font_size"),
            "the line is gone:\n{written}"
        );
        assert_eq!(
            config.tab_width,
            Some(configured.tab_width),
            "the rest is kept"
        );
    }

    #[test]
    fn quitting_writes_a_change_held_for_an_unsaved_init_lua() {
        // The buffer's edits go to the recovery file, not to disk; the change the status line said
        // would be written must not go with nothing.
        use dacode::settings_ui::Field;
        let dir = scratch("settings-quit-held");
        let (mut app, init) = with_init_lua_open(&dir, "-- mine\n");
        app.state.mutate(|document| document.insert("-- edit\n"));
        let _ = app.run(Action::ToggleSettings);
        app.state.focus = dacode::focus::Focus::Editor;
        step_on_page(&mut app, Field::LineNumbers);
        let changed = app.settings_changed_at.unwrap();
        assert!(app.save_settings_if_due(changed + SETTINGS_SAVE_AFTER));
        assert_eq!(std::fs::read_to_string(&init).unwrap(), "-- mine\n", "held");

        app.write_before_leaving();
        let written = std::fs::read_to_string(&init).unwrap();
        assert!(
            written.contains("line_numbers"),
            "written on the way out:\n{written}"
        );
    }

    #[test]
    fn the_wheel_over_the_settings_tab_scrolls_an_open_palette_and_not_the_page() {
        // The palette, help and the menus own the wheel wherever the pointer is. Over the settings
        // tab the page took it first, and scrolled behind the palette it was covered by.
        let dir = scratch("settings-wheel-modal");
        let mut app = with_settings(&dir, dacode::settings_ui::Category::Keys);
        app.open_palette(PaletteKind::Commands);
        let layout = app.layout();
        app.pointer = middle(layout.editor);
        let before = app.state.palette.as_ref().unwrap().selected;

        app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));

        assert_eq!(
            app.state.settings_page.as_ref().unwrap().scroll(),
            0.0,
            "the page did not move"
        );
        assert_ne!(
            app.state.palette.as_ref().unwrap().selected,
            before,
            "the palette did"
        );

        // And with nothing over it, the page takes the wheel.
        app.state.palette = None;
        app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));
        assert!(app.state.settings_page.as_ref().unwrap().scroll() > 0.0);
    }

    #[test]
    fn the_text_behind_the_settings_tab_is_not_edited_while_it_shows() {
        // The document is still there behind the tab, out of sight. An undo or a paste that reached
        // it would change a file nobody can see; a printable key is the search box's.
        let dir = scratch("settings-no-edits");
        let mut app = editor(&dir, "a.rs", "abc");
        app.state.modal = false;
        app.grammar.set_modal(false);
        let _ = app.run(Action::InsertText("x".into()));
        let text = app.state.document.text().to_string();

        let _ = app.run(Action::ToggleSettings);
        app.state.focus = dacode::focus::Focus::Editor;
        let _ = app.run(Action::Undo);
        let _ = app.run(Action::InsertText("y".into()));
        let _ = app.run(Action::DeleteBackward);
        app.press(Key::named(KeyCode::Char('q')));
        assert_eq!(app.state.document.text().to_string(), text);
        assert_eq!(
            app.state
                .settings_page
                .as_ref()
                .map(|page| page.query().to_string()),
            Some("q".to_string()),
            "the key went to the search box"
        );

        // Back on the file, the same keys edit it again.
        app.press(Key::named(KeyCode::Escape));
        app.press(Key::named(KeyCode::Escape));
        assert!(
            !app.state.settings_showing(),
            "escape clears the search, then leaves"
        );
        let _ = app.run(Action::Undo);
        assert_ne!(app.state.document.text().to_string(), text);
    }

    #[test]
    fn closing_the_settings_tab_leaves_the_buffers_and_panes_as_they_were() {
        // The tab is not a buffer. Opening and closing it must not renumber a file, move the live
        // one, or leave a pane pointing at a different file than it was showing.
        let dir = scratch("settings-close");
        std::fs::write(dir.join("b.rs"), "b").unwrap();
        std::fs::write(dir.join("c.rs"), "c").unwrap();
        let mut app = editor(&dir, "a.rs", "a");
        app.headless_size = (WINDOW.width as u32, WINDOW.height as u32);
        app.activate(Target::File(dir.join("b.rs")));
        app.activate(Target::File(dir.join("c.rs")));
        let _ = app.run(Action::SplitPane { vertical: true });
        app.switch_to(1);

        let snapshot = |app: &App| {
            (
                app.state.tab_names(),
                app.state.active_buffer,
                app.state.file_path.clone(),
                app.buffers
                    .iter()
                    .map(|buffer| buffer.file_path.clone())
                    .collect::<Vec<_>>(),
                app.panes.iter().map(|pane| pane.buffer).collect::<Vec<_>>(),
                app.panes.active(),
            )
        };
        let before = snapshot(&app);

        let _ = app.run(Action::ToggleSettings);
        assert_eq!(
            app.state.tab_names().last().map(String::as_str),
            Some("Settings")
        );
        let layout = app.layout();
        let bar = app.tab_bar(layout.tabs, 1.0);
        let tab = bar
            .tabs
            .iter()
            .find(|tab| Some(tab.index) == app.state.settings_tab())
            .expect("on the strip");
        app.pointer = middle(tab.close);
        app.click(false);
        assert!(
            app.state.settings_page.is_none(),
            "its close button closes it"
        );
        assert_eq!(snapshot(&app), before);

        // And ctrl-w, which closes the tab on screen, closes the settings tab rather than a file.
        let _ = app.run(Action::ToggleSettings);
        let _ = app.run(Action::CloseBuffer);
        assert!(app.state.settings_page.is_none());
        assert_eq!(snapshot(&app), before);
    }

    fn commands(app: &App) -> Palette {
        let mut palette = Palette::new(PaletteKind::Commands);
        palette.refresh_commands(app.state.modal);
        palette
    }

    /// The largest alpha among everything drawn on `layer`, or `None` if nothing is.
    fn strongest(frame: &Frame, layer: dacode_render::Layer) -> Option<f32> {
        frame
            .quads_on(layer)
            .map(|quad| quad.color[3])
            .chain(frame.texts_on(layer).map(|run| run.color.alpha))
            .chain(frame.shapes_on(layer).map(|shape| shape.color[3]))
            .reduce(f32::max)
    }

    #[test]
    fn a_palette_at_the_first_instant_of_opening_draws_nothing_and_arrives_soon_after() {
        // Progress zero is invisible, scrim and all — a scrim drawn at full strength under a card
        // that has not yet appeared would flash the window dark for a frame.
        let dir = scratch("palette-arrives");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.palette = Some(commands(&app));
        app.state.sync_motion(WINDOW, 1.0, t0);

        let first = draw_at(&mut app, t0);
        assert!(
            strongest(&first, dacode_render::Layer::Modal).is_none_or(|alpha| alpha <= 0.0),
            "something on the modal layer is visible at progress zero"
        );

        let settled = draw_at(&mut app, ms(t0, 1500));
        assert_eq!(
            strongest(&settled, dacode_render::Layer::Modal),
            Some(1.0),
            "settled, the card is opaque"
        );
    }

    #[test]
    fn sync_sets_every_overlays_target_from_the_state() {
        // The one place targets are set: open each view, sync, and each is heading for open; close
        // them, sync, and each is heading for closed. No site that opens a view starts its motion.
        use dacode::motion::Key as MotionKey;
        let dir = scratch("sync-targets");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let now = std::time::Instant::now();
        let targets = |app: &App| {
            [
                MotionKey::Sidebar,
                MotionKey::Palette,
                MotionKey::Menu,
                MotionKey::Help,
                MotionKey::Completion,
                MotionKey::Info,
                MotionKey::Notifications,
            ]
            .map(|key| app.state.chrome.motion.target(key))
        };

        app.state.sync_motion(WINDOW, 1.0, now);
        assert_eq!(
            targets(&app),
            [Some(0.0); 7],
            "a fresh editor has nothing open"
        );

        app.state.chrome.activity = Some(dacode::layout::Activity::SourceControl);
        app.state.palette = Some(commands(&app));
        app.state.menu = Some(dacode::menu::Menu::bar(app.state.modal));
        app.state.help = Some(dacode::help::Help::new(app.state.modal));
        app.state.completion = Some(dacode::completion::Completion::new(
            app.state.document.text(),
            app.state.cursor(),
            app.state.document.version(),
        ));
        app.state.hover = Some("a hover".into());
        app.state.chrome.show_notifications = true;
        app.state.sync_motion(WINDOW, 1.0, now);
        assert_eq!(targets(&app), [Some(1.0); 7]);

        app.state.close_sidebar();
        app.state.close_palette();
        app.state.close_menu();
        app.state.close_help();
        app.state.completion = None;
        app.state.hover = None;
        app.state.chrome.show_notifications = false;
        app.state.sync_motion(WINDOW, 1.0, ms(now, 16));
        assert_eq!(targets(&app), [Some(0.0); 7]);
    }

    #[test]
    fn reduced_motion_slides_nothing_but_still_fades() {
        // Reduced promises fades only. Half-way through opening, the sidebar is already at its full
        // width — for drawing and for the mouse alike — while the palette is part-faded.
        let dir = scratch("reduced");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        app.state
            .chrome
            .motion
            .set_animations(dacode::motion::Animations::Reduced);
        let t0 = std::time::Instant::now();
        app.state.chrome.activity = Some(dacode::layout::Activity::SourceControl);
        app.state.palette = Some(commands(&app));
        app.state.sync_motion(WINDOW, 1.0, t0);

        let mid = ms(t0, 60);
        let layout =
            dacode::layout::Layout::of(WINDOW, 1.0, None, app.state.sidebar_fraction_at(mid));
        let sidebar = layout.sidebar.expect("open");
        assert_eq!(sidebar.width, layout.sidebar_width, "no slide");

        let frame = draw_at(&mut app, mid);
        let alpha = strongest(&frame, dacode_render::Layer::Modal).expect("fading in");
        assert!(alpha > 0.0 && alpha < 1.0, "part-faded: {alpha}");
        // And it is not growing either: the card is where it will settle.
        let settled = draw_at(&mut app, ms(t0, 2000));
        let card = |frame: &Frame| {
            frame
                .quads_on(dacode_render::Layer::Modal)
                .map(|quad| quad.rect.width)
                .fold(0.0, f32::max)
        };
        assert_eq!(card(&frame), card(&settled));
    }

    #[test]
    fn reduced_motion_asks_for_no_frames_for_movement_it_does_not_show() {
        // Every frame of a sidebar spring under reduced motion would draw the same pixels.
        let dir = scratch("reduced-frames");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        app.state
            .chrome
            .motion
            .set_animations(dacode::motion::Animations::Reduced);
        let t0 = std::time::Instant::now();
        app.state.pointer = (26.0, 80.0);
        app.state.chrome.activity = Some(dacode::layout::Activity::SourceControl);
        app.state.sync_motion(WINDOW, 1.0, t0);
        assert!(!app.animating(ms(t0, 16)));
    }

    #[test]
    fn the_sidebar_overshoot_settles_instead_of_widening_past_its_view() {
        // The view is laid out at the open width; a slot wider than that would reflow it.
        let dir = scratch("overshoot");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.chrome.activity = Some(dacode::layout::Activity::SourceControl);
        app.state.sync_motion(WINDOW, 1.0, t0);
        let widest = (0..200)
            .map(|step| app.state.sidebar_fraction_at(ms(t0, step * 5)))
            .fold(0.0, f32::max);
        assert_eq!(widest, 1.0);
    }

    #[test]
    fn full_motion_slides_the_sidebar_and_the_mouse_follows_the_drawn_frame() {
        let dir = scratch("slide");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.chrome.activity = Some(dacode::layout::Activity::SourceControl);
        app.state.sync_motion(WINDOW, 1.0, t0);
        let mid = ms(t0, 40);
        let fraction = app.state.sidebar_fraction_at(mid);
        assert!(fraction > 0.0 && fraction < 1.0, "part-way: {fraction}");
        // The click handler reads the geometry of the last frame drawn, not of the clock.
        app.last_frame = Some(mid);
        let drawn = dacode::layout::Layout::of(app.window_bounds(), 1.0, None, fraction);
        assert_eq!(app.layout().sidebar, drawn.sidebar);
    }

    #[test]
    fn a_closed_palette_fades_out_without_taking_keys_and_is_then_forgotten() {
        let dir = scratch("palette-leaves");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.palette = Some(commands(&app));
        app.state.sync_motion(WINDOW, 1.0, t0);
        let open = ms(t0, 1500);

        app.state.close_palette();
        assert!(app.state.palette.is_none(), "closed means closed for input");
        app.state.sync_motion(WINDOW, 1.0, open);
        let frame = draw_at(&mut app, ms(t0, 1560));
        let alpha = strongest(&frame, dacode_render::Layer::Modal).expect("still fading out");
        assert!(alpha > 0.0 && alpha < 1.0, "{alpha}");

        app.state.sync_motion(WINDOW, 1.0, ms(t0, 4000));
        assert!(
            app.state.chrome.leaving.palette.is_none(),
            "nothing is kept once it has gone"
        );
        let frame = draw_at(&mut app, ms(t0, 4000));
        assert!(strongest(&frame, dacode_render::Layer::Modal).is_none());
    }

    #[test]
    fn a_modal_that_opens_replaces_one_still_fading_out() {
        // Every modal draws on the same layer, and within a layer all quads are drawn before any
        // glyphs: a menu fading out behind the palette it just opened would print its rows through
        // the palette's card. "Command palette" in the Help menu is exactly that path.
        let dir = scratch("modal-replaces");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.menu = Some(dacode::menu::Menu::bar(app.state.modal));
        app.state.sync_motion(WINDOW, 1.0, t0);
        // The same story without the menu: an editor that only ever opened the palette, at the
        // same instants. The palette lists the same commands in the same words as the menu, so a
        // row cannot be told apart by its text; whatever the first frame has that this one does
        // not is something left behind by the menu.
        let mut alone = editor(&dir, "a.rs", "fn main() {}\n");
        alone.state.sync_motion(WINDOW, 1.0, t0);

        let open = ms(t0, 1500);
        app.state.close_menu();
        app.state.palette = Some(commands(&app));
        app.state.sync_motion(WINDOW, 1.0, open);
        alone.state.palette = Some(commands(&alone));
        alone.state.sync_motion(WINDOW, 1.0, open);

        let modal = |frame: &Frame| -> Vec<String> {
            frame
                .texts_on(dacode_render::Layer::Modal)
                .map(|run| run.text.clone())
                .collect()
        };
        let mut bleeding = modal(&draw_at(&mut app, ms(t0, 1540)));
        for text in modal(&draw_at(&mut alone, ms(t0, 1540))) {
            if let Some(index) = bleeding.iter().position(|drawn| *drawn == text) {
                bleeding.remove(index);
            }
        }
        assert!(
            bleeding.is_empty(),
            "the closed menu's {bleeding:?} is drawn over the palette"
        );
        assert!(app.state.chrome.leaving.menu.is_none());
    }

    #[test]
    fn the_menu_titles_are_always_drawn_and_only_the_dropdown_fades() {
        // The titles used to be part of the menu overlay, so the bar existed only while a menu was
        // open and faded in and out with it. They belong to the title bar now: there when nothing
        // is open, and never on the modal layer where the fading card is.
        use dacode_render::Layer;
        let dir = scratch("menu-titles");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        let texts = |frame: &Frame, layer: Layer| -> Vec<String> {
            frame.texts_on(layer).map(|run| run.text.clone()).collect()
        };

        app.state.sync_motion(WINDOW, 1.0, t0);
        let frame = draw_at(&mut app, t0);
        let base = texts(&frame, Layer::Base);
        for title in dacode::menu::TITLES {
            assert!(
                base.iter().any(|text| text == title),
                "{title} is on the bar"
            );
        }
        assert!(
            !base.iter().any(|text| text == "Share"),
            "the Share button is gone"
        );

        app.state.menu = Some(dacode::menu::Menu::bar(app.state.modal));
        app.state.sync_motion(WINDOW, 1.0, t0);
        let frame = draw_at(&mut app, t0);
        let base = texts(&frame, Layer::Base);
        let modal = texts(&frame, Layer::Modal);
        for title in dacode::menu::TITLES {
            assert!(base.iter().any(|text| text == title), "{title} stays put");
            assert!(
                !modal.iter().any(|text| text == title),
                "{title} is not drawn with the fading card"
            );
        }
    }

    /// GitHub, as far as the updater can tell: the release answer and every file, by URL.
    struct FakeReleases(std::collections::HashMap<String, Vec<u8>>);

    impl dacode::update::Fetch for FakeReleases {
        fn text(&self, url: &str) -> Result<String, String> {
            self.0
                .get(url)
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .ok_or_else(|| format!("nothing at {url}"))
        }
        fn file(&self, url: &str, to: &std::path::Path) -> Result<(), String> {
            let bytes = self.0.get(url).ok_or_else(|| format!("nothing at {url}"))?;
            std::fs::write(to, bytes).map_err(|error| error.to_string())
        }
    }

    /// A release of `version` carrying `files` and their `SHA256SUMS`, served by a fake.
    fn releases(version: &str, files: &[(&str, &[u8])]) -> FakeReleases {
        let url = |name: &str| format!("https://example.invalid/{name}");
        let mut served = std::collections::HashMap::new();
        let mut sums = String::new();
        let mut assets = Vec::new();
        for (name, bytes) in files {
            let mut hasher = dacode::update::Sha256::default();
            hasher.update(bytes);
            sums.push_str(&format!(
                "{}  {name}\n",
                dacode::update::hex(&hasher.finish())
            ));
            served.insert(url(name), bytes.to_vec());
            assets.push(serde_json::json!({ "name": name, "browser_download_url": url(name) }));
        }
        served.insert(url("SHA256SUMS"), sums.into_bytes());
        assets.push(serde_json::json!({
            "name": "SHA256SUMS",
            "browser_download_url": url("SHA256SUMS"),
        }));
        let json = serde_json::json!({
            "tag_name": format!("v{version}"),
            "html_url": format!("https://github.com/Gawad-B/DaCode/releases/tag/v{version}"),
            "draft": false,
            "prerelease": false,
            "assets": assets,
        });
        served.insert(
            dacode::update::LATEST.to_string(),
            json.to_string().into_bytes(),
        );
        FakeReleases(served)
    }

    /// An editor that believes it is `current`, installed as `install`, asking `fake` for releases.
    fn updating(
        dir: &std::path::Path,
        install: dacode::update::Install,
        policy: dacode::update::Policy,
        fake: FakeReleases,
    ) -> App {
        let mut app = editor(dir, "a.rs", "fn main() {}\n");
        app.headless_size = (1280, 820);
        app.install = install;
        app.state.chrome.updates = policy;
        app.updater = dacode::update::Worker::spawn_with(Box::new(fake), "1.0.0", None);
        app
    }

    /// The centre of the offer's button that runs `command`, from the panel's own geometry.
    fn offer_button(app: &App, command: &str) -> (f32, f32) {
        let layout = app.layout();
        let panel = dacode::chrome::notifications_panel(
            &app.state,
            layout.title,
            app.window_bounds(),
            app.scale(),
        );
        let (rect, _) = panel
            .offer
            .expect("an offer on the panel")
            .buttons
            .into_iter()
            .find(|(_, button)| button.command == command)
            .unwrap_or_else(|| panic!("no {command} button"));
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    #[test]
    fn a_newer_release_is_offered_in_the_bell_and_skipping_it_is_remembered() {
        use dacode::update::{Install, Policy};
        let dir = scratch("update-offer");
        let fake = releases("1.2.0", &[("dacode-1.2.0-x86_64-linux.tar.gz", b"tar")]);
        let prefix = dir.join("prefix");
        let mut app = updating(&dir, Install::Tarball { prefix }, Policy::Notify, fake);

        let _ = app.run(Action::CheckForUpdates);
        assert!(settle(&mut app, |app| app.state.chrome.update.is_some()));
        let offer = app.state.chrome.update.clone().unwrap();
        assert_eq!(offer.message, "DaCode 1.2.0 is available");
        let labels: Vec<_> = offer.buttons.iter().map(|button| button.label).collect();
        assert_eq!(labels, ["Update", "Release notes", "Skip this version"]);
        // Asked for by hand, so the answer is put in front of the person who asked.
        assert!(app.state.chrome.show_notifications);
        assert!(
            app.state
                .chrome
                .notifications
                .iter()
                .any(|message| message.contains("1.2.0")),
            "{:?}",
            app.state.chrome.notifications
        );

        // A click on the drawn button, through the same routing every click takes.
        app.pointer = offer_button(&app, "skip_update");
        app.state.pointer = app.pointer;
        app.click(false);
        assert!(app.state.chrome.update.is_none(), "the offer is gone");
        assert_eq!(
            dacode::update::skipped(&app.update_dir).as_deref(),
            Some("1.2.0")
        );

        // The daily check does not bring a skipped version back.
        app.updater
            .request(dacode::update::Request::Check { manual: false });
        assert!(settle(&mut app, |app| !app.updater.busy()));
        assert!(app.state.chrome.update.is_none());
        assert!(app.pending_update.is_none());
    }

    #[test]
    fn an_automatic_update_is_downloaded_verified_and_offered_as_a_restart() {
        use dacode::update::{Install, Policy, Stage};
        let dir = scratch("update-ready");
        let fake = releases("1.2.0", &[("dacode-setup.exe", b"setup")]);
        let mut app = updating(&dir, Install::WindowsInstaller, Policy::Automatic, fake);

        let _ = app.run(Action::CheckForUpdates);
        assert!(settle(&mut app, |app| {
            app.pending_update
                .as_ref()
                .is_some_and(|pending| matches!(pending.stage, Stage::Ready(_)))
        }));
        let offer = app.state.chrome.update.clone().unwrap();
        assert_eq!(offer.message, "DaCode 1.2.0 is ready — restart to update");
        assert_eq!(offer.buttons[0].label, "Restart now");
        assert_eq!(
            std::fs::read(app.update_dir.join("1.2.0").join("dacode-setup.exe")).unwrap(),
            b"setup"
        );
    }

    #[test]
    fn restarting_to_update_with_unsaved_work_refuses_once_as_quitting_does() {
        // Restart is a quit, and a quit with three dirty buffers must not be one click from
        // leaving them. Refused before anything is written or installed.
        use dacode::update::{Install, Pending, Policy, Stage, Staged};
        let dir = scratch("update-unsaved");
        let fake = releases("1.2.0", &[]);
        let mut app = updating(&dir, Install::WindowsInstaller, Policy::Automatic, fake);
        app.pending_update = Some(Pending {
            release: dacode::update::parse_release(
                r#"{"tag_name": "v1.2.0", "html_url": "https://x", "assets": []}"#,
            )
            .unwrap(),
            stage: Stage::Ready(Staged {
                version: "1.2.0".into(),
                installer: dir.join("never-run.exe"),
            }),
        });
        let _ = app.run(Action::InsertText("x".into()));
        assert!(app.state.document.is_dirty());

        let _ = app.run(Action::RestartToUpdate);
        assert!(app.restart_requested);
        app.restart_requested = false;
        assert!(!app.restart_for_update(), "refused");
        assert!(app.confirm_quit, "armed for the second press");
        let said = app.state.hover.clone().unwrap_or_default();
        assert!(said.contains("1 unsaved file"), "{said}");
        assert!(said.contains("Restart now again"), "{said}");
    }

    #[test]
    fn the_startup_check_runs_once_a_day_and_never_for_a_build_or_with_updates_off() {
        use dacode::update::{Install, Policy};
        let dir = scratch("update-daily");
        let fake = releases("1.0.0", &[]);
        let prefix = dir.join("prefix");
        let mut app = updating(&dir, Install::Tarball { prefix }, Policy::Automatic, fake);

        app.check_for_updates_if_due();
        assert!(app.updater.busy(), "asked");
        assert!(settle(&mut app, |app| !app.updater.busy()));
        assert!(app.state.chrome.update.is_none(), "1.0.0 is not newer");
        // Up to date, and not asked: nothing said.
        assert!(app.state.hover.is_none());
        app.check_for_updates_if_due();
        assert!(!app.updater.busy(), "not again the same day");

        for (install, policy) in [
            (Install::Cargo, Policy::Automatic),
            (Install::WindowsInstaller, Policy::Off),
        ] {
            let other = scratch("update-never");
            let mut app = updating(&other, install, policy, releases("1.0.0", &[]));
            app.check_for_updates_if_due();
            assert!(!app.updater.busy());
            assert_eq!(dacode::update::last_check(&app.update_dir), None);
        }

        // With updates off, the command still asks.
        let other = scratch("update-off-command");
        let mut app = updating(
            &other,
            Install::WindowsInstaller,
            Policy::Off,
            releases("1.0.0", &[]),
        );
        let _ = app.run(Action::CheckForUpdates);
        assert!(settle(&mut app, |app| !app.updater.busy()));
        assert_eq!(
            app.state.hover.as_deref(),
            Some(concat!(
                "DaCode ",
                env!("CARGO_PKG_VERSION"),
                " is the latest version"
            ))
        );
    }

    #[test]
    fn a_message_swings_the_bell() {
        let dir = scratch("bell");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.chrome.now = Some(t0);
        app.state.chrome.notify("saved");
        let swing = app
            .state
            .chrome
            .motion
            .get(dacode::motion::Key::Bell, ms(t0, 70));
        assert!(swing.abs() > 0.5, "swinging: {swing}");
        assert!(
            !app.animating(ms(t0, 3000)),
            "and still again within seconds"
        );
    }

    #[test]
    fn a_tab_opened_grows_in_and_the_active_background_glides_to_it() {
        use dacode::motion::Key as MotionKey;
        let dir = scratch("tabs");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");
        let t0 = std::time::Instant::now();
        app.state.buffer_count = 1;
        app.state.active_buffer = 0;
        app.state.sync_motion(WINDOW, 1.0, t0);
        assert_eq!(
            app.state.chrome.motion.get(MotionKey::TabIndicator, t0),
            0.0
        );

        app.state.buffer_count = 2;
        app.state.active_buffer = 1;
        app.state.sync_motion(WINDOW, 1.0, ms(t0, 16));
        let motion = &app.state.chrome.motion;
        let glide = motion.get(MotionKey::TabIndicator, ms(t0, 60));
        assert!(glide > 0.0 && glide < 1.0, "between the two tabs: {glide}");
        let grow = motion.get(MotionKey::TabGrow, ms(t0, 40));
        assert!(grow > 0.0 && grow < 1.0, "growing: {grow}");
    }

    #[test]
    fn the_loop_asks_for_frames_while_something_moves_and_sleeps_once_it_stops() {
        // The editor has no frame timer any more: it is woken by workers and by input. Motion is the
        // one thing that needs a frame *without* either, so `animating` is what keeps it drawing — and
        // the moment it is false the loop must go back to `Wait`, or an idle editor burns a core.
        use dacode::motion::{Key as MotionKey, Spring};
        let dir = scratch("animating");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        let now = std::time::Instant::now();
        assert!(!app.animating(now), "nothing is moving in a fresh editor");

        app.state
            .chrome
            .motion
            .to(MotionKey::Sidebar, 1.0, Spring::PANEL, now);
        assert!(app.animating(now + std::time::Duration::from_millis(16)));
        assert!(
            !app.animating(now + std::time::Duration::from_secs(2)),
            "a settled spring must let the loop sleep"
        );
        assert_eq!(
            app.control_flow(now + std::time::Duration::from_secs(2)),
            ControlFlow::Wait,
            "and sleeping means Wait, not a poll"
        );
    }

    #[test]
    fn a_redraw_leaves_the_clock_to_the_wall_clock_for_whatever_runs_between_frames() {
        // `state.now()` is what an input handler would start a spring with. If a frame's instant stayed
        // set, a keypress after an idle minute would retarget a spring a minute in the past, and the
        // next frame would find it already arrived: the panel would jump instead of moving.
        let dir = scratch("frame-clock");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        app.state.chrome.now = Some(std::time::Instant::now() - std::time::Duration::from_secs(60));
        let before = std::time::Instant::now();
        app.redraw();
        assert!(
            app.state.now() >= before,
            "state.now() is still the time of an old frame"
        );
        assert_eq!(app.state.chrome.now, None);
    }

    #[test]
    fn nothing_asks_for_frames_for_a_window_nobody_can_see() {
        // A hidden window's redraw returns without presenting, so nothing paces the loop to vsync and a
        // request per idle step is a busy loop for as long as the spring takes to settle.
        use dacode::motion::{Key as MotionKey, Spring};
        let dir = scratch("occluded");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        let now = std::time::Instant::now();
        app.state
            .chrome
            .motion
            .to(MotionKey::Sidebar, 1.0, Spring::PANEL, now);
        let mid = now + std::time::Duration::from_millis(16);

        app.occluded = true;
        assert!(!app.wants_frame(mid), "occluded");
        app.occluded = false;
        assert!(app.wants_frame(mid), "visible again, and still moving");
    }

    #[test]
    fn the_overflow_with_nothing_hidden_opens_nothing_and_an_unknown_title_is_an_error() {
        // `--menu-title …` at a width where every title fits used to list only the last one, and a
        // misspelt title quietly opened File: both captured a menu the editor would never show.
        use dacode::menu::TITLES;
        assert!(menu_for_title("…", TITLES.len(), false).unwrap().is_none());
        assert!(
            menu_for_title("...", TITLES.len(), false)
                .unwrap()
                .is_none()
        );
        let list = menu_for_title("…", TITLES.len() - 2, false)
            .unwrap()
            .expect("two are hidden, so there is a list");
        assert!(list.listing);
        assert_eq!(list.items.len(), 2);

        let edit = menu_for_title("edit", TITLES.len(), false)
            .unwrap()
            .unwrap();
        assert_eq!(TITLES[edit.open], TITLES[1]);
        let error = menu_for_title("Flie", TITLES.len(), false).expect_err("not a title");
        assert!(format!("{error:#}").contains("Flie"), "{error:#}");
    }

    #[test]
    fn a_bad_at_ms_is_an_error_not_a_silent_default() {
        // Someone reproducing a mid-transition frame who typo'd the number would otherwise get a
        // different frame and no hint why. Reached before any GPU is touched, so this needs no device.
        let args: Vec<String> = ["--at-ms", "abc", "--empty"].map(String::from).to_vec();
        let error = screenshot(&args, "/nonexistent/never-written.png")
            .expect_err("a non-number is refused");
        assert!(format!("{error:#}").contains("--at-ms"), "{error:#}");
    }

    #[test]
    fn the_frame_after_the_last_one_that_moved_is_asked_for_too() {
        // `busy` turns false once a value is within half a pixel of its target, but the frame drawn
        // just before that was still a hair short of it. Without one more frame the editor would sleep
        // showing a panel a fraction of a pixel from where it is meant to be.
        use dacode::motion::{Key as MotionKey, Spring};
        let dir = scratch("animating-last-frame");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        let now = std::time::Instant::now();
        app.state
            .chrome
            .motion
            .to(MotionKey::Sidebar, 1.0, Spring::PANEL, now);

        assert!(app.wants_frame(now + std::time::Duration::from_millis(16)));
        let later = now + std::time::Duration::from_secs(2);
        assert!(app.wants_frame(later), "one settling frame after the move");
        assert!(!app.wants_frame(later), "and then none");
    }

    #[test]
    fn a_dirty_autosaving_buffer_sets_an_alarm_for_the_end_of_the_quiet_period() {
        // With a waker the loop sleeps until input. Autosave waits on the *clock*, which nothing
        // wakes the loop for — with a language server running it used to ride the 100 ms tick, and
        // the tick is gone. Without this the save waits for an unrelated keypress.
        let dir = scratch("autosave-alarm");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        app.state.autosave = true;
        let now = std::time::Instant::now();
        assert_eq!(
            app.control_flow(now),
            ControlFlow::Wait,
            "a clean buffer needs nothing"
        );

        app.state.mutate(|document| document.insert("x"));
        app.last_edit = Some(now);
        assert_eq!(
            app.control_flow(now),
            ControlFlow::WaitUntil(now + AUTOSAVE_QUIET)
        );

        app.state.autosave = false;
        assert_eq!(
            app.control_flow(now),
            ControlFlow::Wait,
            "and nothing when autosave is off"
        );

        app.state.autosave = true;
        assert_eq!(
            app.control_flow(now + AUTOSAVE_QUIET),
            ControlFlow::Wait,
            "a deadline already passed would spin the loop: what is still dirty then was refused"
        );
    }

    #[test]
    fn with_a_waker_only_the_walk_and_a_search_keep_a_timer() {
        // The contract that lets an idle editor with a language server sleep. `formatting_version`
        // stands in for any worker that owes an answer: with a waker it announces itself.
        let dir = scratch("busy-contract");
        let mut app = woken_editor(&dir, "a.rs", "fn main() {}\n");
        app.formatting_version = Some(Version(1));
        assert!(
            !app.services_busy(),
            "a worker with a waker must not keep the loop polling"
        );

        app.walk = Some(FileWalk::start(dir.clone()));
        assert!(app.services_busy(), "the walk has no waker");
        app.walk = None;

        // And without a waker, the old clauses still apply: nobody would announce the answer.
        app.waker = None;
        assert!(app.services_busy(), "a format in flight must be waited for");
        app.formatting_version = None;
        assert!(!app.services_busy());
    }

    /// Colours arrive from the idle step alone — no key, no click. They used to wait for one.
    #[test]
    fn highlights_arrive_without_any_input() {
        let dir = scratch("idle-colours");
        let mut app = editor(&dir, "a.rs", "fn main() { let x = 1; }\n");
        // `resumed` starts the worker in the real editor; a test has no window to resume.
        app.syntax = Some(SyntaxWorker::spawn(Language::Rust).expect("the Rust grammar loads"));
        app.sync_syntax();
        assert!(settle(&mut app, |app| !app.state.highlights.is_empty()));
    }

    /// The span for a token, as a parse would hand it over.
    fn span(range: std::ops::Range<usize>) -> dacode_syntax::HighlightSpan {
        dacode_syntax::HighlightSpan {
            range: CharOffset(range.start)..CharOffset(range.end),
            highlight: dacode_syntax::Highlight::Type,
        }
    }

    /// What a span names, so a test asserts about text rather than about numbers.
    fn spanned(app: &App, index: usize) -> String {
        let span = &app.state.highlights[index];
        app.state
            .document
            .text()
            .slice(span.range.clone())
            .to_string()
    }

    /// The same editor, with its recovery directory inside the test's own.
    fn recovering(dir: &std::path::Path, name: &str, contents: &str) -> App {
        let mut app = editor(dir, name, contents);
        app.recovery = Recovery::in_dir(dir.join("recovery"));
        app
    }

    fn type_text(app: &mut App, text: &str) {
        execute(
            &mut app.state,
            &mut app.registers,
            &Action::InsertText(text.into()),
            40,
        );
    }

    /// `ctrl-s` saves through the same path as `:w`.
    ///
    /// It reached the executor's own `Save` arm, which wrote the file and nothing else: the recovery
    /// copy stayed behind and was offered again on the next open, as though the save had never
    /// happened, and `format_on_save` never ran for the key most people actually press.
    #[test]
    fn the_save_key_clears_the_recovery_copy_like_any_other_save() {
        let dir = scratch("save-key");
        let mut app = recovering(&dir, "a.rs", "fn main() {}\n");
        type_text(&mut app, "// edited\n");
        app.persist_recovery(true);
        let file = dir.join("a.rs");
        // Asked of the path, not of `find`: that drops a copy no newer than the file, and the file
        // was written a moment ago — inside one tick of the kernel's clock on a loaded machine.
        assert!(
            app.recovery.path_for(&file).exists(),
            "the edit has a recovery copy"
        );

        app.run(Action::Save);

        assert!(!app.state.document.is_dirty());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            app.state.document.text().to_string()
        );
        assert!(
            !app.recovery.path_for(&file).exists(),
            "a saved file's recovery copy must go, or it is offered again as unsaved work"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A save does not silently overwrite a change another program made since the file was opened.
    ///
    /// The watcher only says so for the active buffer, and only on the hover line, which the next
    /// keystroke replaces. Nothing checked at the moment it matters, so a `git checkout`, a
    /// formatter run in a terminal, or a teammate's edit on a shared mount was undone by the next
    /// save without a word.
    #[test]
    fn a_save_does_not_overwrite_a_change_made_on_disk() {
        let dir = scratch("conflict");
        let file = dir.join("a.rs");
        let mut app = recovering(&dir, "a.rs", "fn main() {}\n");
        type_text(&mut app, "// mine\n");
        std::fs::write(&file, "fn main() { theirs() }\n").unwrap();

        app.run(Action::Save);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "fn main() { theirs() }\n",
            "the first save is refused, and says why"
        );
        assert!(
            app.state.document.is_dirty(),
            "and the buffer is still unsaved"
        );

        app.run(Action::Save);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            app.state.document.text().to_string(),
            "asking again, having been told, overwrites — an editor that cannot save is worse"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Autosave is never the thing that overwrites someone else's change: it fires because typing
    /// paused, not because anyone chose to.
    #[test]
    fn autosave_never_overwrites_a_change_made_on_disk() {
        let dir = scratch("conflict-auto");
        let file = dir.join("a.rs");
        let mut app = recovering(&dir, "a.rs", "fn main() {}\n");
        type_text(&mut app, "// mine\n");
        std::fs::write(&file, "fn main() { theirs() }\n").unwrap();

        app.write_active(Saving::Auto);
        app.write_active(Saving::Auto);

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "fn main() { theirs() }\n"
        );
        assert!(
            app.state.prompt.is_none(),
            "autosave reports on the hover line; a prompt would take the next keystroke"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A file the editor itself rewrote, or one reloaded from disk, is not a conflict.
    #[test]
    fn saving_twice_in_a_row_is_not_a_conflict() {
        let dir = scratch("no-conflict");
        let file = dir.join("a.rs");
        let mut app = recovering(&dir, "a.rs", "fn main() {}\n");
        type_text(&mut app, "// one\n");
        app.run(Action::Save);
        type_text(&mut app, "// two\n");
        app.run(Action::Save);

        assert!(
            !app.state.document.is_dirty(),
            "the second save went through first time"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            app.state.document.text().to_string()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// What the panic hook will write is what is on screen, not what the last timed write saw.
    #[test]
    fn every_dirty_buffer_is_held_for_the_panic_hook() {
        let dir = scratch("lifeline");
        let file = dir.join("a.rs");
        let mut app = recovering(&dir, "a.rs", "fn main() {}\n");
        app.hold_lifeline();
        assert_eq!(app.lifeline.flush(), 0, "nothing is unsaved yet");

        type_text(&mut app, "// unsaved\n");
        app.hold_lifeline();

        assert_eq!(app.lifeline.flush(), 1);
        // Read directly rather than through `find`, which drops a copy written in the same clock
        // tick as the file — and this one is, on a loaded machine.
        let written = std::fs::read_to_string(app.recovery.path_for(&file))
            .expect("the flush wrote a recovery copy");
        assert!(written.ends_with(&app.state.document.text().to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Highlight spans move with the text they describe, rather than staying where they were.
    ///
    /// The spans are only ever set from a parse whose version matched the buffer, and that was taken
    /// to mean they could never be stale. The next keystroke makes them stale, and the parse that
    /// replaces them takes a few milliseconds to arrive — so every span after the cursor named text
    /// that had already moved. Typing a hundred characters into a line rendered the rest of the file
    /// in the colors of the text a hundred characters earlier, and it corrected itself as soon as
    /// typing stopped, which is what made it look like a rendering fault.
    #[test]
    fn highlight_spans_move_with_the_text_they_describe() {
        let dir = scratch("spans-move");
        let mut app = editor(&dir, "a.py", "import os\nx = 1\n");
        app.state.document.set_primary_caret(CharOffset(0));
        app.state.highlights = vec![span(7..9)];
        assert_eq!(spanned(&app, 0), "os", "the span names `os` to begin with");

        execute(
            &mut app.state,
            &mut app.registers,
            &Action::InsertText("abc".into()),
            40,
        );

        assert_eq!(
            spanned(&app, 0),
            "os",
            "after typing three characters ahead of it, the span still names `os`"
        );
    }

    /// A span whose text is deleted is dropped, not left describing whatever slid into its place.
    #[test]
    fn a_span_whose_text_is_deleted_is_dropped() {
        let dir = scratch("spans-deleted");
        let mut app = editor(&dir, "a.py", "import os\nx = 1\n");
        app.state.document.set_primary_caret(CharOffset(9));
        app.state.highlights = vec![span(7..9)];

        for _ in 0..2 {
            execute(
                &mut app.state,
                &mut app.registers,
                &Action::DeleteBackward,
                40,
            );
        }

        assert!(
            app.state.highlights.is_empty(),
            "the two characters the span named are gone, so the span is too"
        );
    }

    /// A diagnostic's underline stays under the code it is about when text moves ahead of it.
    ///
    /// The same defect as the highlight spans and the longest-lived of the three: a diagnostic costs
    /// a round trip to another process, so the window in which its range describes text that has
    /// already moved is as long as the server takes to answer, not the milliseconds a parse takes.
    /// The ranges are line-and-column rather than offsets, which is why `Diagnostics::transform`
    /// needs the text from both sides of the edit — a position cannot be moved without knowing which
    /// document it is a position in.
    #[test]
    fn a_diagnostic_underlines_the_code_it_is_about_after_an_edit() {
        use dacode_lsp::lsp_types::{Diagnostic, Position, Range};

        let dir = scratch("diagnostic-moves");
        let mut app = editor(&dir, "a.py", "import os\nx = 1\n");
        app.state.document.set_primary_caret(CharOffset(0));

        // An error over `os`, as a server would report it: line 0, characters 7 to 9.
        let at = |character| Position { line: 0, character };
        app.state.diagnostics.apply(
            vec![Diagnostic {
                range: Range {
                    start: at(7),
                    end: at(9),
                },
                message: "unused import".into(),
                ..Default::default()
            }],
            None,
            app.state.document.version(),
        );

        execute(
            &mut app.state,
            &mut app.registers,
            &Action::InsertText("abc".into()),
            40,
        );

        let text = app.state.document.text().clone();
        let range = dacode_lsp::range_from_lsp(&text, app.state.diagnostics.items()[0].range);
        assert_eq!(
            text.slice(range).to_string(),
            "os",
            "the underline still sits under the import it is about"
        );
    }

    /// A file created outside the editor appears without the folder being opened again.
    ///
    /// The reported bug, and it was structural rather than a slip: the index is built by exactly one
    /// walk, at startup, and nothing ever asked for a second. The watcher existed but watched only
    /// the directories of *open* files and reported only paths the editor already had open, so a
    /// file arriving from another program, a `git checkout` or a browser download was invisible to
    /// the tree and the file picker until the folder was reopened.
    ///
    /// Drives the same idle work the event loop drives, rather than a copy of it.
    #[test]
    fn a_file_created_outside_the_editor_appears_without_reopening_the_folder() {
        let dir = scratch("appears");
        let mut app = editor(&dir, "a.rs", "fn main() {}\n");

        // The startup walk, which is also what registers the project's directories to watch.
        app.walk = Some(FileWalk::start(dir.clone()));
        assert!(
            idle_until(&mut app, |app| app.index.files().len() == 1),
            "the first walk should index the one file that exists"
        );

        std::fs::write(dir.join("added-by-something-else.rs"), "fn other() {}\n").unwrap();

        assert!(
            idle_until(&mut app, |app| app
                .index
                .files()
                .iter()
                .any(|file| file.relative == "added-by-something-else.rs")),
            "a file created outside the editor should be indexed without reopening the folder"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Runs the idle work the event loop runs, until `done` or a deadline.
    fn idle_until(app: &mut App, mut done: impl FnMut(&App) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            app.poll_walk();
            app.rewalk_if_project_changed();
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// A program that behaves like a formatter: reads stdin, writes stdout.
    fn tidy_formatter() -> Option<dacode::format::Configured> {
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "more".to_string()])
        } else {
            ("cat", Vec::new())
        };
        dacode_lsp::resolve(program)?;
        dacode::format::configure("text", program, &args, &["txt".to_string()]).ok()
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
        if dacode_lsp::resolve(program).is_none() {
            eprintln!("skipping: no {program} on this machine");
            return;
        }
        let formatter =
            dacode::format::configure("text", program, &args, &["txt".to_string()]).unwrap();

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
            dacode::format::configure(
                "text",
                "dacode-no-such-formatter",
                &[],
                &["txt".to_string()],
            )
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
        if dacode::process::command("git")
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
            dacode::process::command("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        std::fs::write(dir.join("notes.txt"), "one\ntwo\nthree\n").unwrap();
        if git(&["init", "-q"]).is_none()
            || git(&["config", "user.email", "dacode@example.invalid"]).is_none()
            || git(&["config", "user.name", "dacode"]).is_none()
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

    /// A repository at `dir` with `notes.txt` committed, or `None` (and a note) where git is not
    /// available — the skip CLAUDE.md asks for, rather than a failure that tests the machine.
    fn committed_notes(dir: &std::path::Path, contents: &str) -> Option<()> {
        if dacode::process::command("git")
            .arg("--version")
            .output()
            .map(|out| !out.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: no git on this machine");
            return None;
        }
        let git = |args: &[&str]| {
            dacode::process::command("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        std::fs::write(dir.join("notes.txt"), contents).unwrap();
        let built = git(&["init", "-q"]).is_some()
            && git(&["config", "user.email", "ann@example.invalid"]).is_some()
            && git(&["config", "user.name", "Ann Example"]).is_some()
            && git(&["add", "."]).is_some()
            && git(&[
                "commit",
                "-q",
                "-m",
                "write the notes",
                "-m",
                "With a body.",
            ])
            .is_some();
        if !built {
            eprintln!("skipping: could not build a repository here");
        }
        built.then_some(())
    }

    #[test]
    fn blame_is_toggled_by_its_keys_and_fills_a_column_from_the_real_repository() {
        // The whole path: a key, through the grammar and the executor, to the event loop asking
        // git, to a column the layout reserves — in both grammars, since the column is a feature
        // and a feature reachable only from Vim's keys is not one a conventional user has.
        let dir = scratch("blame-keys");
        if committed_notes(&dir, "one\ntwo\nthree\n").is_none() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        let mut app = editor(&dir, "notes.txt", "one\ntwo\nthree\n");
        app.state.modal = true;
        app.grammar.set_modal(true);
        app.state.mode = Mode::Normal;

        assert!(!app.state.blame.column);
        app.press(Key::char('g'));
        app.press(Key::char('b'));
        assert!(app.state.blame.column, "gb turns the column on");
        assert!(
            settle(&mut app, |app| app.state.blame.blame().is_some()),
            "the blame never arrived"
        );
        assert!(matches!(
            app.state.blame.at(1),
            Some(dacode::blame::Attribution::Commit(commit)) if commit.author == "Ann Example"
        ));
        let scale = app.scale();
        assert!(
            app.layout().pane(0, 1, true, scale).blame.is_some(),
            "and the layout the mouse reads has the column the drawing has"
        );

        // `gB`: the whole commit, body and all, in the info card.
        app.press(Key::char('g'));
        app.press(Key::char('B'));
        assert!(
            settle(&mut app, |app| app
                .state
                .hover
                .as_ref()
                .is_some_and(|text| text.contains("With a body."))),
            "the commit never arrived: {:?}",
            app.state.hover
        );

        // The conventional editor's spelling turns it off again.
        app.state.modal = false;
        app.grammar.set_modal(false);
        app.state.mode = Mode::Insert;
        app.press(Key {
            code: KeyCode::Char('b'),
            ctrl: false,
            alt: true,
            shift: false,
        });
        assert!(!app.state.blame.column, "alt-b turns it off");
        assert!(app.layout().pane(0, 1, true, scale).blame.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_caret_lines_blame_follows_typing_and_yields_to_a_diagnostic() {
        // Through the real edit path, so it is `mutate` that keeps the blame right: a line added
        // above must move every attribution below it, or the note names the commit of the line that
        // used to be there.
        let dir = scratch("blame-inline");
        let mut app = editor(&dir, "notes.txt", "one\ntwo\nthree\n");
        let commit = dacode::blame::Commit {
            hash: "a".repeat(40),
            author: "Ann Example".into(),
            mail: String::new(),
            time: 1_000_000,
            summary: "write the notes".into(),
        };
        let other = dacode::blame::Commit {
            hash: "b".repeat(40),
            author: "Bob Example".into(),
            ..commit.clone()
        };
        app.state.blame.inline = true;
        app.state.blame.show(dacode::blame::Blame::new(
            dir.join("notes.txt"),
            dacode::blame::Parsed {
                commits: vec![commit, other],
                lines: vec![0, 1, 0],
            },
        ));
        let now = 1_000_000 + 3 * 86_400;

        app.state.document.set_primary_caret(CharOffset(4));
        assert_eq!(
            chrome::line_note(&app.state, now),
            Some(chrome::LineNote::Blame(
                "Bob Example, 3 days ago · write the notes".into()
            ))
        );

        app.state.document.set_primary_caret(CharOffset(0));
        app.press(Key::char('x'));
        app.press(Key::named(KeyCode::Enter));
        assert_eq!(
            app.state.document.text().to_string(),
            "x\none\ntwo\nthree\n"
        );
        assert!(
            matches!(
                app.state.blame.at(2),
                Some(dacode::blame::Attribution::Commit(c)) if c.author == "Bob Example"
            ),
            "`two` moved down a line and kept its author"
        );
        assert_eq!(
            chrome::line_note(&app.state, now),
            Some(chrome::LineNote::Blame("not committed yet".into())),
            "the caret's line was touched by the edit"
        );

        // A problem on the line takes the place. A blame is something to know; an error is
        // something to act on, and hiding it behind its author would be the wrong way round.
        let version = app.state.document.version();
        let line = app.state.cursor_point().line as u32;
        app.state.diagnostics.apply(
            vec![dacode_lsp::lsp_types::Diagnostic {
                range: dacode_lsp::lsp_types::Range {
                    start: dacode_lsp::lsp_types::Position { line, character: 0 },
                    end: dacode_lsp::lsp_types::Position { line, character: 3 },
                },
                message: "unused".into(),
                ..Default::default()
            }],
            None,
            version,
        );
        assert!(matches!(
            chrome::line_note(&app.state, now),
            Some(chrome::LineNote::Diagnostic { message, .. }) if message == "unused"
        ));

        // And the setting turns it off without touching the diagnostic's turn.
        app.state.diagnostics.clear();
        app.state.blame.inline = false;
        assert_eq!(chrome::line_note(&app.state, now), None);

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
        app.git.request(dacode::git::DiffRequest {
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
        let dir =
            std::env::temp_dir().join(format!("dacode-buffers-{}-{label}", std::process::id()));
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
            &dacode::config::Settings::default(),
            None,
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
    fn ctrl_w_closes_the_file_on_screen_in_insert_and_normal_mode() {
        // Through `App::press`, the path a real key takes: the grammar resolves ctrl-w to
        // `CloseBuffer` and the event loop closes the live buffer. In insert mode too, where Vim's
        // own ctrl-w deletes a word — the key means one thing in DaCode, in every mode.
        let dir = scratch("ctrl-w");
        let mut app = editor(&dir, &["a.rs", "b.rs", "c.rs"]);
        assert_eq!(app.state.mode, Mode::Insert);
        app.press(Key::ctrl('w'));
        assert_eq!(names(&app), ["a.rs", "b.rs"], "the live file closed");
        assert!(
            !app.state.document.text().to_string().contains('w'),
            "and nothing was typed"
        );

        app.state.set_mode(Mode::Normal);
        app.press(Key::ctrl('w'));
        assert_eq!(names(&app).len(), 1, "and again from normal mode");
        assert!(
            app.grammar.is_idle(),
            "a whole command, not a prefix waiting"
        );
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
        // `dacode_project::canonical` hands back its input unchanged once `canonicalize` fails, and it
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

    #[test]
    fn a_file_opened_from_the_empty_editor_replaces_untitled() {
        // The empty scratch buffer is "nothing open yet". Kept, it sat in the strip as `untitled`
        // beside the first real file, and closing that file brought it back.
        let dir = scratch("placeholder");
        std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        let (state, _) = new_state(
            Text::from_str(""),
            vec!["untitled".into()],
            None,
            Mode::Insert,
            None,
            dir.to_path_buf(),
        );
        let mut app = App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &dacode::config::Settings::default(),
            None,
        );
        assert!(app.state.tab_names().is_empty(), "no tab for nothing");

        app.activate(Target::File(dir.join("a.rs")));
        assert!(
            app.buffers.is_empty(),
            "the placeholder is gone, not stored"
        );
        assert_eq!(app.state.tab_names(), ["a.rs"]);
    }

    #[test]
    fn changing_folder_closes_what_is_saved_and_keeps_what_is_not() {
        // As VSCode does — but a buffer with unsaved work stays rather than being discarded.
        let dir = scratch("folder-change");
        let mut app = editor(&dir, &["a.rs", "b.rs", "c.rs"]);
        app.switch_to(1);
        app.state.document.mark_dirty();

        let kept = app.close_clean_buffers();
        assert_eq!(kept, 1);
        assert_eq!(names(&app), ["b.rs"]);
        assert_eq!(app.state.tab_names(), ["b.rs"]);
    }

    #[test]
    fn the_problems_shown_follow_the_file_on_screen() {
        // The panel and the counts are the live file's: the ones from a file left behind used to
        // stay in the list, and in the badge, whatever was open.
        use dacode_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
        let dir = scratch("problems-follow");
        let mut app = editor(&dir, &["a.rs", "b.rs"]);
        let error = |line| Diagnostic {
            range: Range {
                start: Position { line, character: 0 },
                end: Position { line, character: 1 },
            },
            severity: Some(DiagnosticSeverity::ERROR),
            message: "broken".into(),
            ..Default::default()
        };
        app.state
            .problems
            .publish(&dir.join("a.rs"), &[error(0), error(1)], None);
        app.state
            .problems
            .publish(&dir.join("b.rs"), &[error(0)], None);

        assert_eq!(app.state.problems.counts(), (1, 0), "b.rs is live");
        app.switch_to(0);
        assert_eq!(app.state.problems.counts(), (2, 0), "and now a.rs is");
        assert!(
            app.state
                .problems
                .ordered()
                .iter()
                .all(|problem| problem.path.ends_with("a.rs"))
        );
    }

    #[test]
    fn up_and_down_keep_their_column_across_a_short_line() {
        // Without a goal the column was recomputed from wherever the last move had clamped, so one
        // blank line on the way left the caret in the first column for the rest of the walk.
        let dir = scratch("goal-column");
        let mut app = editor(&dir, &["a.rs"]);
        let text = "let first = 1;\n\nlet third = 3;\n\nlet fifth;\n";
        app.state.document = Document::from_str(text);
        let line = |app: &App| app.state.cursor_point();

        // From the end of a line: the end of every line reached.
        app.state.document.set_primary_caret(CharOffset(14));
        for _ in 0..2 {
            app.press(Key::named(KeyCode::Down));
        }
        assert_eq!(
            line(&app),
            dacode_text::Point::new(2, 14),
            "end of `let third = 3;`"
        );
        for _ in 0..2 {
            app.press(Key::named(KeyCode::Down));
        }
        assert_eq!(
            line(&app),
            dacode_text::Point::new(4, 10),
            "end of the shorter `let fifth;`"
        );

        // From the middle: that column, once the line is long enough again.
        app.state.document.set_primary_caret(CharOffset(4));
        for _ in 0..2 {
            app.press(Key::named(KeyCode::Down));
        }
        assert_eq!(line(&app), dacode_text::Point::new(2, 4));
    }

    #[test]
    fn ctrl_shift_f_opens_the_search_view_and_typing_goes_to_its_query() {
        // `ctrl-f` is this file; `ctrl-shift-f` is the search view, which then has the keys, so
        // the next letter is part of the query rather than an edit to the code.
        let dir = scratch("search-view");
        let mut app = editor(&dir, &["a.rs"]);
        let before = app.state.document.text().to_string();
        app.press(Key {
            code: KeyCode::Char('f'),
            ctrl: true,
            alt: false,
            shift: true,
        });
        assert!(app.state.search_showing());
        assert_eq!(app.state.focus, dacode::focus::Focus::Sidebar);
        let query = app.state.search.as_ref().unwrap().query.clone();

        app.press(Key::char('x'));
        assert_eq!(
            app.state.search.as_ref().unwrap().query,
            format!("{query}x")
        );
        assert_eq!(app.state.document.text().to_string(), before);

        app.press(Key {
            code: KeyCode::Char('r'),
            ctrl: false,
            alt: true,
            shift: false,
        });
        assert!(
            app.state.search.as_ref().unwrap().regex,
            "alt-r is the regex toggle"
        );
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dacode-session-{}-{label}", std::process::id()));
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
            &dacode::config::Settings::default(),
            None,
        );
        for name in &names[1..] {
            app.activate(Target::File(dir.join(name)));
        }
        app
    }

    fn file_names(session: &dacode::session::Session) -> Vec<String> {
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

/// The wheel, the touchpad and the pinch, driven through the handlers `window_event` and
/// `about_to_wait` call, on an editor with no window.
#[cfg(test)]
mod scroll_tests {
    use super::*;
    use std::time::{Duration, Instant};
    use winit::dpi::PhysicalPosition;

    fn editor(label: &str) -> App {
        let dir =
            std::env::temp_dir().join(format!("dacode-scroll-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let contents: String = (0..400).map(|i| format!("let line_{i} = {i};\n")).collect();
        let path = dir.join("long.rs");
        std::fs::write(&path, &contents).unwrap();
        let (state, _) = new_state(
            Text::from_str(&contents),
            vec!["long.rs".to_string()],
            Some(path.to_string_lossy().to_string()),
            Mode::Insert,
            None,
            dir.clone(),
        );
        App::new(
            dir,
            state,
            Grammar::new(),
            &dacode::config::Settings::default(),
            None,
        )
    }

    /// A touchpad's delta: `down` pixels of the content moving up.
    fn swipe(app: &mut App, down: f32) {
        app.settle_scroll_input();
        app.mouse_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            0.0,
            -f64::from(down),
        )));
    }

    #[test]
    fn a_touchpad_stream_of_small_deltas_scrolls_the_file() {
        // The report: "I can't scroll with the touchpad". Every 3-pixel delta was divided by forty
        // and truncated, so the file did not move at all however far the fingers went.
        let mut app = editor("stream");
        let row = app.row_height_px();
        for _ in 0..40 {
            swipe(&mut app, 3.0);
        }
        let whole = (120.0 / row).floor();
        assert_eq!(app.state.scroll.line, whole as usize);
        assert!(
            (app.state.sub_row_px(row) - (120.0 - whole * row)).abs() < 1e-3,
            "the part of a row left over is drawn, not dropped"
        );
    }

    #[test]
    fn a_wheel_notch_glides_and_lands_on_a_whole_row() {
        let mut app = editor("glide");
        let row = app.row_height_px();
        // Part-scrolled first, which is where a glide could most easily end a fraction of a row off.
        swipe(&mut app, row * 0.4);
        let start = Instant::now();
        app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));
        assert_eq!(
            app.state.scroll.line, 0,
            "a notch glides rather than jumping"
        );

        app.advance_glide(start + Duration::from_millis(40));
        let travelled = app.state.scroll.line as f32 * row + app.state.sub_row_px(row);
        assert!(
            travelled > row * 0.4 && travelled < row * 3.0,
            "part-way, {travelled} px down"
        );

        app.advance_glide(start + Duration::from_secs(2));
        assert_eq!(app.state.scroll.line, 3, "three rows a notch");
        assert_eq!(app.state.sub_row_px(row), 0.0, "and on a whole row");
        assert!(!app.state.chrome.motion.busy(start + Duration::from_secs(2)));
    }

    #[test]
    fn a_touchpad_overtakes_a_glide_in_flight() {
        let mut app = editor("overtake");
        let start = Instant::now();
        app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));
        app.advance_glide(start + Duration::from_millis(30));
        let line = app.state.scroll.line;

        swipe(&mut app, 1.0);
        app.advance_glide(start + Duration::from_secs(2));
        assert!(
            app.state.scroll.line <= line + 1,
            "the rest of the notch's travel is abandoned, not played out under the fingers"
        );
    }

    #[test]
    fn moving_the_cursor_drops_the_part_scrolled_row() {
        // Scrolling that follows the cursor is never offset: a caret on a row half under the tab bar
        // is a caret nobody can see.
        let mut app = editor("cursor");
        let row = app.row_height_px();
        swipe(&mut app, row * 2.5);
        assert!(app.state.sub_row_px(row) > 0.0);

        app.press(Key::named(KeyCode::Down));
        app.settle_scroll_input();
        assert_eq!(app.state.sub_row_px(row), 0.0);
    }

    #[test]
    fn fractional_line_deltas_add_up_to_scrolling() {
        // X11's smooth scrolling and Windows' precision touchpads send a touchpad as fractions of a
        // line. Rounded to whole lines per event, a tenth of a line times three was 0.3 and became
        // nothing — the touchpad bug again, on every platform but Wayland.
        let mut app = editor("fraction");
        let row = app.row_height_px();
        for _ in 0..40 {
            app.settle_scroll_input();
            app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, -0.1));
        }
        app.advance_glide(Instant::now() + Duration::from_secs(2));
        let travelled = app.state.scroll.line as f32 * row + app.state.sub_row_px(row);
        assert!(
            (travelled - 12.0 * row).abs() < 1.0,
            "forty tenths of a line, at three rows a line, is twelve rows; went {travelled} px"
        );
    }

    #[test]
    fn small_ctrl_deltas_add_up_to_one_zoom_step() {
        // With a step per event, a flick that sends twenty tiny deltas ran the type from smallest
        // to largest. A line's worth of them is one step, as one notch of a mouse wheel is.
        let mut app = editor("zoom");
        let size = app.state.typography.code_size;
        app.modifiers = ModifiersState::CONTROL;
        for _ in 0..19 {
            app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, 0.05));
        }
        assert_eq!(
            app.state.typography.code_size, size,
            "under a line: no step yet"
        );
        app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, 0.05));
        let once = app.state.typography.code_size;
        assert!(once > size, "a whole line is a step");
        let _ = app.run(Action::DecreaseFontSize);
        assert_eq!(
            app.state.typography.code_size, size,
            "exactly one step, not several"
        );
    }

    #[test]
    fn a_ctrl_notch_the_other_way_is_not_swallowed_by_a_leftover_fraction() {
        // A touchpad flick up leaves part of a step behind; the next notch down must still be a
        // whole step, not a step minus that leftover.
        let mut app = editor("zoom reverse");
        app.modifiers = ModifiersState::CONTROL;
        for _ in 0..12 {
            app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, 0.05));
        }
        let size = app.state.typography.code_size;
        app.mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));
        assert!(
            app.state.typography.code_size < size,
            "a whole notch down shrinks the type"
        );
    }

    #[test]
    fn clicking_another_pane_ignores_the_part_scrolled_row() {
        // The remainder belongs to the pane that was scrolled. Read against the pane just clicked —
        // drawn on whole rows because it was unfocused — it moved the click a row down whenever the
        // pointer was in the lower part of a row.
        let mut app = editor("panes");
        let _ = app.run(Action::SplitPane { vertical: true });
        let row = app.row_height_px();
        swipe(&mut app, row * 0.5);
        assert!(app.state.sub_row_px(row) > 0.0);

        let other = 1 - app.panes.active();
        let code = app.layout().pane(other, 2, true, app.scale()).code;
        app.pointer = (code.x + code.width / 2.0, code.y + row * 0.75);
        app.settle_scroll_input();
        app.click(false);

        assert_eq!(app.panes.active(), other);
        let line = app.state.document.text().char_to_line(app.state.cursor());
        assert_eq!(
            line, app.state.scroll.line,
            "the click lands on the first row of the pane it was made in"
        );
    }

    #[test]
    fn a_bare_modifier_leaves_the_view_where_it_was() {
        // Ctrl, Shift and Alt arrive as key presses that resolve to nothing, and the key path then
        // revealed the cursor: holding ctrl to zoom or to ctrl-click a name threw the view back to
        // wherever the caret was, and the click landed on whatever was now under the pointer.
        let mut app = editor("modifier");
        let row = app.row_height_px();
        swipe(&mut app, row * 40.5);
        let scroll = app.state.scroll;
        let offset = app.state.sub_row_px(row);

        for key in [NamedKey::Control, NamedKey::Shift, NamedKey::Alt] {
            app.settle_scroll_input();
            assert!(!app.key_event(WinitKey::Named(key)));
            app.settle_scroll_input();
        }
        assert_eq!(app.state.scroll, scroll);
        assert_eq!(app.state.sub_row_px(row), offset);
    }

    #[test]
    fn the_row_sliding_in_at_the_bottom_is_highlighted_too() {
        // While part-scrolled, one row more is drawn than fits whole. Asking the highlighter for
        // the whole rows only left that row in plain ink for as long as it was sliding in.
        let mut app = editor("highlight");
        let row = app.row_height_px();
        swipe(&mut app, row * 10.5);
        let rows = app.code_rows();
        let last_drawn = app.state.scroll.line + rows;
        let text = app.state.document.text();
        assert!(
            app.visible_range().end >= text.line_range(last_drawn).end,
            "the highlighted range reaches line {last_drawn}"
        );
    }

    #[test]
    fn pinching_steps_the_font_size_a_tenth_at_a_time() {
        let mut app = editor("pinch");
        let size = app.state.typography.code_size;

        app.pinch(0.06);
        assert_eq!(
            app.state.typography.code_size, size,
            "under a tenth does nothing yet"
        );
        app.pinch(0.06);
        let larger = app.state.typography.code_size;
        assert!(larger > size, "the two together are a step");

        app.pinch(-0.25);
        assert!(app.state.typography.code_size < larger);
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::*;
    use dacode::focus::Focus;
    use std::time::{Duration, Instant};

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dacode-terminal-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor in insert mode with one file open, whose terminal starts the platform's own shell
    /// — `sh`, or `cmd` on Windows — and never the developer's `$SHELL`.
    fn editor(dir: &std::path::Path) -> App {
        let path = dir.join("a.txt");
        std::fs::write(&path, "hello\n").unwrap();
        let (state, _) = new_state(
            Text::from_str("hello\n"),
            vec!["a.txt".to_string()],
            Some(path.to_string_lossy().to_string()),
            Mode::Insert,
            None,
            dir.to_path_buf(),
        );
        let mut app = App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &dacode::config::Settings::default(),
            None,
        );
        app.config_dir = dir.join("config");
        app.terminal_shells = Some(vec![if cfg!(windows) {
            dacode_terminal::ShellCommand::new("cmd", &[])
        } else {
            dacode_terminal::ShellCommand::new("sh", &[])
        }]);
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            let key = if c == '\r' {
                Key::named(KeyCode::Enter)
            } else {
                Key::char(c)
            };
            app.press(key);
        }
    }

    fn settle(app: &mut App, mut done: impl FnMut(&App) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            app.collect_background();
            if done(app) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn shows(app: &App, wanted: &[&str]) -> bool {
        app.active_terminal().is_some_and(|terminal| {
            terminal
                .screen()
                .lines()
                .iter()
                .any(|line| wanted.contains(&line.trim()))
        })
    }

    /// Ends the shell and proves it ended: no test may leave one behind.
    fn end(app: &mut App) {
        for terminal in app.terminals.drain(..) {
            assert!(
                terminal.shutdown(Duration::from_secs(10)),
                "the shell's thread ended"
            );
        }
    }

    #[test]
    fn keys_go_to_the_shell_only_while_the_terminal_has_focus() {
        // PLAN.md's warning, as a test through the real key path: a terminal done badly is a shell
        // that eats your keystrokes. With the terminal focused every key is the shell's and the
        // document does not change; with the editor focused — by ctrl-` or by clicking the code —
        // every key is the document's and the shell is sent nothing.
        let dir = scratch("routing");
        let mut app = editor(&dir);
        let text = |app: &App| app.state.document.text().to_string();

        app.press(Key::ctrl('`'));
        assert!(app.state.terminal.open, "ctrl-` opens the terminal");
        assert_eq!(app.state.focus, Focus::Terminal, "and gives it the keys");
        assert!(app.active_terminal().is_some(), "with a shell in it");

        type_text(&mut app, "echo routed$((6*7))\r");
        let ran = settle(&mut app, |app| shows(app, &["routed42", "routed$((6*7))"]));
        assert_eq!(text(&app), "hello\n", "nothing typed reached the document");
        let sent = app.active_terminal().unwrap().bytes_sent();
        assert!(sent > 0);

        // Out by the same key: the panel hides, the text has the keys again.
        app.press(Key::ctrl('`'));
        assert_eq!(app.state.focus, Focus::Editor);
        assert!(!app.state.terminal.open, "hidden");
        type_text(&mut app, "x");
        assert_eq!(
            text(&app),
            "xhello\n",
            "the editor's key reached the document"
        );
        assert_eq!(app.active_terminal().unwrap().bytes_sent(), sent);

        // In again, and out by clicking the code: the escape hatch that needs no key.
        app.press(Key::ctrl('`'));
        assert_eq!(app.state.focus, Focus::Terminal);
        let editor = app.layout().editor;
        app.pointer = (editor.x + 200.0, editor.y + 10.0);
        app.click(false);
        assert_eq!(
            app.state.focus,
            Focus::Editor,
            "clicking the code takes the keys back"
        );
        assert!(app.state.terminal.open, "and leaves the terminal showing");
        let before = app.active_terminal().unwrap().bytes_sent();
        type_text(&mut app, "y");
        assert!(text(&app).contains('y'), "{:?}", text(&app));
        assert_eq!(app.active_terminal().unwrap().bytes_sent(), before);

        // And clicking the terminal hands the keys back to it.
        let panel = app.layout().terminal.expect("open");
        app.pointer = (panel.x + 40.0, panel.bottom() - 10.0);
        app.click(false);
        assert_eq!(app.state.focus, Focus::Terminal);

        end(&mut app);
        assert!(ran, "the command ran in the shell");
    }

    #[test]
    fn the_editors_shortcuts_are_the_shells_while_it_has_focus() {
        // ctrl-w deletes a word in a shell and closes a buffer in the editor. In the terminal it
        // must be the first: the editor keeps only the few keys `dacode::terminal::route` names.
        let dir = scratch("shortcuts");
        let mut app = editor(&dir);
        app.press(Key::ctrl('`'));
        let tabs = app.state.buffer_tabs.len();
        for key in [
            Key::ctrl('w'),
            Key::ctrl('p'),
            Key::ctrl('s'),
            Key::ctrl('e'),
        ] {
            app.press(key);
        }
        assert_eq!(app.state.buffer_tabs.len(), tabs, "no buffer closed");
        assert!(app.state.palette.is_none(), "ctrl-p is the shell's history");
        assert!(
            app.state.tree.is_none(),
            "ctrl-e is the end of the shell's line"
        );
        assert_eq!(app.state.focus, Focus::Terminal);

        // ctrl-shift-p is the editor's, and the palette it opens owns the keys while it is open.
        app.press(Key::ctrl('p').with_shift());
        assert!(app.state.palette.is_some(), "the command palette opens");
        let sent = app.active_terminal().unwrap().bytes_sent();
        type_text(&mut app, "ab");
        assert_eq!(
            app.active_terminal().unwrap().bytes_sent(),
            sent,
            "typed into the palette, not the shell under it"
        );
        app.press(Key::named(KeyCode::Escape));
        assert!(app.state.palette.is_none());
        assert_eq!(
            app.state.focus,
            Focus::Terminal,
            "and the keys go back where they were"
        );
        end(&mut app);
    }

    #[test]
    fn ctrl_t_opens_the_terminal_and_closes_it_again_from_inside() {
        // The second key for the terminal, through `App::press`. It has to work from inside as
        // well as out — the route keeps it from the shell — or it would open a panel that only
        // ctrl-` could leave.
        let dir = scratch("ctrl-t");
        let mut app = editor(&dir);
        app.press(Key::ctrl('t'));
        assert!(app.state.terminal.open, "ctrl-t opens the terminal");
        assert_eq!(app.state.focus, Focus::Terminal, "and gives it the keys");
        assert!(app.active_terminal().is_some(), "with a shell in it");
        let sent = app.active_terminal().unwrap().bytes_sent();

        app.press(Key::ctrl('t'));
        assert!(!app.state.terminal.open, "and closes it from inside");
        assert_eq!(app.state.focus, Focus::Editor);
        assert_eq!(
            app.active_terminal().unwrap().bytes_sent(),
            sent,
            "the shell was not sent a transpose"
        );
        assert_eq!(app.state.document.text().to_string(), "hello\n");
        end(&mut app);
    }

    #[test]
    fn exit_closes_the_terminal_and_gives_the_keys_back() {
        let dir = scratch("exit");
        let mut app = editor(&dir);
        app.press(Key::ctrl('`'));
        type_text(&mut app, "exit\r");
        let closed = settle(&mut app, |app| app.terminals.is_empty());
        assert!(closed, "the shell's exit was noticed");
        assert!(!app.state.terminal.open);
        assert_eq!(app.state.focus, Focus::Editor);
        assert!(app.state.hover.is_none(), "a clean exit needs no message");
    }

    #[test]
    fn the_panels_buttons_add_a_tab_and_hide_it() {
        // Mouse-reachable both ways, through the same rectangles the header is drawn from. The +
        // starts another shell beside the first rather than replacing it: whatever ran in the
        // first is still there to switch back to.
        let dir = scratch("buttons");
        let mut app = editor(&dir);
        app.window_action(&Action::FocusTerminal);
        let rects = app.terminal_rects().expect("shown");

        app.pointer = (rects.new.x + 4.0, rects.new.y + 4.0);
        let first = app.active_terminal().unwrap().bytes_sent();
        type_text(&mut app, "echo old\r");
        assert!(app.active_terminal().unwrap().bytes_sent() > first);
        app.click(false);
        assert_eq!(app.terminals.len(), 2, "a second shell");
        assert_eq!(app.state.terminal.active, 1, "shown");
        assert_eq!(app.state.terminal.tabs.len(), 2, "with a tab each");
        assert_eq!(
            app.active_terminal().map(|terminal| terminal.bytes_sent()),
            Some(0),
            "a fresh shell"
        );
        assert!(app.terminals[0].bytes_sent() > first, "the first kept");

        app.pointer = (rects.close.x + 4.0, rects.close.y + 4.0);
        app.click(false);
        assert!(!app.state.terminal.open, "hidden");
        assert_eq!(app.state.focus, Focus::Editor);
        assert_eq!(app.terminals.len(), 2, "and both still running");
        end(&mut app);
    }

    #[test]
    fn tabs_switch_by_click_and_key_and_closing_the_last_hides_the_panel() {
        // The tab lifecycle through the real paths: the header's own rectangles for the mouse,
        // ctrl-pagedown and ctrl-pageup from inside the terminal, each tab's close ending its shell,
        // and the last one going taking the panel with it while keeping its height for next time.
        let dir = scratch("tabs");
        let mut app = editor(&dir);
        app.window_action(&Action::FocusTerminal);
        app.window_action(&Action::NewTerminal);
        app.window_action(&Action::NewTerminal);
        assert_eq!(app.terminals.len(), 3);
        assert_eq!(app.state.terminal.active, 2);

        let rects = app.terminal_rects().expect("shown");
        let tabs = dacode::layout::terminal_tabs(
            &rects,
            &app.state.terminal.tabs,
            &app.state.typography,
            app.scale(),
        );
        assert_eq!(tabs.len(), 3);
        app.pointer = (
            tabs[0].label.x + 2.0,
            tabs[0].label.y + tabs[0].label.height / 2.0,
        );
        app.click(false);
        assert_eq!(app.state.terminal.active, 0, "a click shows that tab");
        assert_eq!(app.state.focus, Focus::Terminal);

        app.press(Key::parse_sequence("<c-pageup>")[0]);
        assert_eq!(
            app.state.terminal.active, 2,
            "ctrl-pageup wraps round to the last"
        );
        app.press(Key::parse_sequence("<c-pagedown>")[0]);
        assert_eq!(app.state.terminal.active, 0, "and ctrl-pagedown back");
        assert_eq!(
            app.state.document.text().to_string(),
            "hello\n",
            "the document was never typed into"
        );

        // Closing the shown tab shows its neighbour; closing one before it keeps what is shown.
        app.pointer = (tabs[0].close.x + 2.0, tabs[0].close.y + 2.0);
        app.click(false);
        assert_eq!(app.terminals.len(), 2);
        assert_eq!(app.state.terminal.active, 0, "the neighbour slid in");
        app.window_action(&Action::NextTerminal);
        assert_eq!(app.state.terminal.active, 1);
        app.window_action(&Action::KillTerminal);
        assert_eq!(app.terminals.len(), 1);
        assert_eq!(
            app.state.terminal.active, 0,
            "the last tab closed, so the one before"
        );

        app.state.terminal.fraction = 0.55;
        app.window_action(&Action::KillTerminal);
        assert!(app.terminals.is_empty());
        assert!(!app.state.terminal.open, "the last one hides the panel");
        assert_eq!(app.state.focus, Focus::Editor, "and gives the keys back");
        assert_eq!(app.state.terminal.fraction, 0.55, "its height is kept");
        end(&mut app);
    }

    #[test]
    fn a_drag_on_the_grid_selects_and_ctrl_shift_c_copies_it() {
        // Mouse selection end to end: a press on a cell and a drag along the row, through the same
        // `cell_at` the drawing's `cell` inverts, then the copy key from inside the terminal. The
        // shell prints a known word at a known place, so which cells were dragged over is known.
        let dir = scratch("select");
        let mut app = editor(&dir);
        app.window_action(&Action::FocusTerminal);
        // No `clear` first: it is a different command in each of the shells the panel may start,
        // and the row is found by its whole text anyway, which the typed command never matches.
        type_text(&mut app, "echo selectable\r");
        let ready = settle(&mut app, |app| shows(app, &["selectable"]));
        assert!(
            ready,
            "the word was printed: {:?}",
            app.active_terminal().map(|t| t.screen().lines())
        );
        let screen = app.active_terminal().unwrap().screen();
        let row = (0..screen.rows.len())
            .find(|row| screen.row_text(*row).trim() == "selectable")
            .expect("on screen");

        let rects = app.terminal_rects().expect("shown");
        let first = rects.cell(row, 0);
        let last = rects.cell(row, "selectable".len() - 1);
        app.pointer = (first.x + 1.0, first.y + first.height / 2.0);
        app.click_count = 1;
        app.dragging = true;
        app.terminal_gesture = None;
        app.click(false);
        assert_eq!(app.terminal_gesture, Some(TerminalGesture::Select));
        app.pointer = (last.right() - 1.0, last.y + last.height / 2.0);
        app.drag_terminal();
        app.release_terminal(dacode_terminal::MouseButton::Left);
        assert_eq!(app.terminal_gesture, None);
        assert_eq!(
            app.active_terminal().unwrap().selection_text().as_deref(),
            Some("selectable")
        );
        let sent = app.active_terminal().unwrap().bytes_sent();
        app.press(Key::ctrl('c').with_shift());
        assert_eq!(
            app.active_terminal().unwrap().bytes_sent(),
            sent,
            "ctrl-shift-c is the editor's copy, not the shell's interrupt"
        );
        assert!(app.state.hover.is_none(), "{:?}", app.state.hover);

        // A right-click over the panel offers the terminal's menu, copy first, and not the code's.
        app.open_context_menu();
        let menu = app.state.menu.as_ref().expect("a menu opened");
        assert_eq!(menu.items[0].action, Action::TerminalCopy);
        end(&mut app);
    }

    #[test]
    fn a_program_that_asked_for_the_mouse_gets_the_click_and_shift_still_selects() {
        // Through the real press path: once the program turns SGR reporting on, a press on the grid
        // is sent to it as `ESC [ < 0 ; x ; y M` and selects nothing, its release follows, and a
        // shift-press selects as it would with no program asking — xterm's way out.
        let dir = scratch("mouse");
        let mut app = editor(&dir);
        app.window_action(&Action::FocusTerminal);
        type_text(&mut app, "printf '\\033[?1000h\\033[?1006h'\r");
        let asked = settle(&mut app, |app| {
            app.active_terminal()
                .is_some_and(|terminal| terminal.mouse_modes().reporting())
        });
        assert!(asked, "the shell turned reporting on");

        let rects = app.terminal_rects().expect("shown");
        let cell = rects.cell(2, 4);
        app.pointer = (cell.x + 1.0, cell.y + 1.0);
        app.click_count = 1;
        let before = app.active_terminal().unwrap().bytes_sent();
        app.terminal_gesture = None;
        app.click(false);
        app.release_terminal(dacode_terminal::MouseButton::Left);
        let sent = app.active_terminal().unwrap().bytes_sent() - before;
        assert_eq!(
            sent,
            (b"\x1b[<0;5;3M".len() + b"\x1b[<0;5;3m".len()) as u64,
            "a press and its release, one-based"
        );
        assert!(!app.active_terminal().unwrap().has_selection());

        app.modifiers = ModifiersState::SHIFT;
        let before = app.active_terminal().unwrap().bytes_sent();
        app.click(false);
        assert_eq!(app.terminal_gesture, Some(TerminalGesture::Select));
        assert_eq!(app.active_terminal().unwrap().bytes_sent(), before);
        app.modifiers = ModifiersState::empty();
        end(&mut app);
    }

    #[test]
    fn dragging_the_edge_resizes_the_panel_and_the_shells_grid() {
        // The edge is grabbed through its own rectangle and follows the pointer; the grid the
        // shell is told it has follows the panel, as `fit_terminal` promises.
        let dir = scratch("resize");
        let mut app = editor(&dir);
        app.window_action(&Action::FocusTerminal);
        app.collect_background();
        let before = app.terminal_rects().expect("shown");
        let rows = app.active_terminal().unwrap().size().rows;

        app.state.focus = Focus::Editor;
        app.pointer = (before.panel.x + 300.0, before.panel.y);
        app.dragging = true;
        app.terminal_gesture = None;
        app.click(false);
        assert_eq!(app.terminal_gesture, Some(TerminalGesture::Resize));
        assert_eq!(
            app.state.focus,
            Focus::Editor,
            "grabbing the edge moved no keys"
        );
        app.pointer.1 -= 120.0;
        app.drag_terminal();
        app.release_terminal(dacode_terminal::MouseButton::Left);

        let after = app.terminal_rects().expect("shown");
        assert!(
            (after.panel.y - (before.panel.y - 120.0)).abs() <= 1.0,
            "the edge followed the pointer: {} to {}",
            before.panel.y,
            after.panel.y
        );
        assert_eq!(after.panel.bottom(), before.panel.bottom());
        app.collect_background();
        let grid = app.active_terminal().unwrap().size();
        assert!(grid.rows > rows, "{} rows, then {}", rows, grid.rows);
        assert_eq!((grid.columns, grid.rows), (after.columns, after.rows));

        // And the height is part of the session.
        let fraction = app.state.terminal.fraction;
        let session = app.session_snapshot();
        assert_eq!(session.terminal_fraction, Some(fraction));
        end(&mut app);
    }

    #[test]
    fn the_shell_is_told_the_size_of_the_grid_on_screen() {
        for size in [(640, 480), (1280, 820), (1920, 1080)] {
            let dir = scratch(&format!("size-{}", size.0));
            let mut app = editor(&dir);
            app.headless_size = size;
            app.window_action(&Action::FocusTerminal);
            app.collect_background();
            let rects = app.terminal_rects().expect("shown");
            let grid = app.active_terminal().unwrap().size();
            assert_eq!(
                (grid.columns, grid.rows),
                (rects.columns, rects.rows),
                "{size:?}"
            );
            end(&mut app);
        }
    }
}

/// The source-control view driven through the editor's own paths: its keys, its clicks, the
/// worker, and a real repository in a directory of each test's own.
#[cfg(test)]
mod source_control_tests {
    use super::*;
    use dacode::scm::Group;
    use dacode::source_control::{Field, Row};

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dacode-scm-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> Option<String> {
        dacode::process::command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// A repository with `notes.txt` committed, or `None` where git is missing — a skip, as
    /// CLAUDE.md asks, not a failure that tests the machine. Signing and hooks are turned off in the
    /// repository itself, so a developer's global config cannot make a commit here ask for a key.
    /// Line-ending conversion is off for the same reason: Git for Windows defaults to
    /// `core.autocrlf=true` and put back `one\r\ntwo\r\n` where `one\ntwo\n` was committed.
    fn repository(label: &str) -> Option<std::path::PathBuf> {
        if git(std::path::Path::new("."), &["--version"]).is_none() {
            eprintln!("skipping: no git on this machine");
            return None;
        }
        let dir = scratch(label);
        std::fs::write(dir.join("notes.txt"), "one\ntwo\n").unwrap();
        std::fs::create_dir_all(dir.join(".git-hooks-none")).unwrap();
        let built = git(&dir, &["init", "-q"]).is_some()
            && git(&dir, &["config", "user.email", "ann@example.invalid"]).is_some()
            && git(&dir, &["config", "user.name", "Ann Example"]).is_some()
            && git(&dir, &["config", "commit.gpgsign", "false"]).is_some()
            && git(&dir, &["config", "core.hooksPath", ".git-hooks-none"]).is_some()
            && git(&dir, &["config", "core.autocrlf", "false"]).is_some()
            && git(&dir, &["add", "notes.txt"]).is_some()
            && git(&dir, &["commit", "-q", "-m", "write the notes"]).is_some();
        if !built {
            eprintln!("skipping: could not build a repository here");
            return None;
        }
        Some(dir)
    }

    fn editor(dir: &std::path::Path, name: &str, contents: &str) -> App {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        let (state, _) = new_state(
            Text::from_str(contents),
            vec![name.to_string()],
            Some(path.to_string_lossy().to_string()),
            Mode::Insert,
            None,
            dir.to_path_buf(),
        );
        let mut app = App::new(
            dir.to_path_buf(),
            state,
            Grammar::new(),
            &dacode::config::Settings::default(),
            None,
        );
        app.config_dir = dir.join("config");
        app.state.modal = false;
        app.grammar.set_modal(false);
        app
    }

    fn settle(app: &mut App, mut done: impl FnMut(&App) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            app.collect_background();
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// Lets every answer still on its way land, so the next assertion reads a settled view.
    fn quiet(app: &mut App) {
        settle(app, |app| !app.scm.busy());
    }

    fn row_of(app: &App, group: Group, path: &str) -> Option<usize> {
        app.state.scm.rows().iter().position(|row| match row {
            Row::File { group: at, entry } => {
                *at == group
                    && app
                        .state
                        .scm
                        .status()
                        .map(|s| s.entries[*entry].path.as_str())
                        == Some(path)
            }
            Row::Header(_) => false,
        })
    }

    fn type_keys(app: &mut App, text: &str) {
        for c in text.chars() {
            app.press(Key::char(c));
        }
    }

    fn ctrl_enter() -> Key {
        Key {
            ctrl: true,
            ..Key::named(KeyCode::Enter)
        }
    }

    #[test]
    fn stage_unstage_and_commit_from_the_views_own_keys() {
        // The whole loop someone runs in VSCode's view, with the keys this one documents: open it,
        // type a message, stage a file, change their mind, stage it again, commit.
        let Some(dir) = repository("loop") else {
            return;
        };
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let mut app = editor(&dir, "notes.txt", "one\ntwo\nthree\n");

        app.press(Key::ctrl('g').with_shift());
        assert!(app.state.source_control_showing(), "ctrl-shift-g opens it");
        assert_eq!(app.state.focus, dacode::focus::Focus::Sidebar);
        assert_eq!(
            app.state.scm.field,
            Field::Message,
            "with the caret in the box"
        );
        assert!(
            settle(&mut app, |app| app.state.scm.count(Group::Changes) == 2),
            "the status never arrived: {:?}",
            app.state.scm.state
        );
        assert!(app.state.scm.status().unwrap().branch.is_some());

        type_keys(&mut app, "Add a third note");
        assert_eq!(app.state.scm.message, "Add a third note");
        assert_eq!(
            app.state.document.text().to_string(),
            "one\ntwo\nthree\n",
            "typing went to the box, not the file behind it"
        );

        app.press(Key::named(KeyCode::Tab));
        assert_eq!(app.state.scm.field, Field::List);
        app.state.scm.selected = row_of(&app, Group::Changes, "notes.txt");
        app.press(Key::char('s'));
        assert!(
            settle(&mut app, |app| app.state.scm.count(Group::Staged) == 1),
            "s never staged it"
        );
        quiet(&mut app);

        app.state.scm.selected = row_of(&app, Group::Staged, "notes.txt");
        app.press(Key::char('u'));
        assert!(
            settle(&mut app, |app| app.state.scm.count(Group::Staged) == 0),
            "u never unstaged it"
        );
        quiet(&mut app);

        app.state.scm.selected = row_of(&app, Group::Changes, "notes.txt");
        app.press(Key::char(' '));
        assert!(settle(&mut app, |app| app.state.scm.count(Group::Staged) == 1));
        quiet(&mut app);

        app.press(ctrl_enter());
        assert!(
            settle(&mut app, |app| app.state.scm.count(Group::Staged) == 0
                && app.state.scm.busy.is_none()),
            "the commit never landed"
        );
        quiet(&mut app);
        assert_eq!(
            git(&dir, &["log", "-1", "--format=%s"]).unwrap().trim(),
            "Add a third note"
        );
        assert!(
            app.state.scm.message.is_empty(),
            "the box empties on success"
        );
        assert_eq!(
            app.state.scm.count(Group::Changes),
            1,
            "new.txt was never staged, so it is still a change"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn committing_with_nothing_staged_asks_once_then_commits_every_change() {
        // VSCode's smart commit: rather than refusing, it offers to stage everything — and here the
        // offer is the first press, so nothing is committed that was not asked for twice.
        let Some(dir) = repository("smart") else {
            return;
        };
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let mut app = editor(&dir, "notes.txt", "one\ntwo\nthree\n");
        app.press(Key::ctrl('g').with_shift());
        assert!(settle(&mut app, |app| app.state.scm.count(Group::Changes) == 2));
        type_keys(&mut app, "Everything");

        app.press(ctrl_enter());
        assert_eq!(
            app.state.scm.confirm,
            Some(dacode::source_control::Confirm::StageAllAndCommit)
        );
        quiet(&mut app);
        assert_eq!(
            git(&dir, &["rev-list", "--count", "HEAD"]).unwrap().trim(),
            "1",
            "the first press only asks"
        );

        app.press(ctrl_enter());
        assert!(settle(&mut app, |app| {
            app.state.scm.busy.is_none() && app.state.scm.count(Group::Changes) == 0
        }));
        quiet(&mut app);
        assert_eq!(
            git(&dir, &["rev-list", "--count", "HEAD"]).unwrap().trim(),
            "2"
        );
        assert_eq!(git(&dir, &["status", "--porcelain"]).unwrap(), "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discarding_asks_twice_then_puts_a_file_back_and_deletes_an_untracked_one() {
        // The only buttons in the view that throw work away, so each has to be pressed twice — and
        // pressing it on a different file in between starts the question again.
        let Some(dir) = repository("discard") else {
            return;
        };
        std::fs::write(dir.join("scratch.txt"), "throwaway\n").unwrap();
        let mut app = editor(&dir, "notes.txt", "one\nCHANGED\n");
        app.press(Key::ctrl('g').with_shift());
        assert!(settle(&mut app, |app| app.state.scm.count(Group::Changes) == 2));
        quiet(&mut app);

        // By the row's own ↶ button, where the mouse would press it.
        let sidebar = app.layout().sidebar.expect("the view is open");
        let press_discard = |app: &mut App, path: &str| {
            let row = row_of(app, Group::Changes, path).expect("listed");
            let geometry = app.state.scm.geometry(sidebar, 1.0, &app.state.typography);
            let rect = geometry
                .rows
                .iter()
                .find(|(index, _)| *index == row)
                .map(|(_, rect)| *rect)
                .expect("on screen");
            let (_, button) = app
                .state
                .scm
                .action_rects(row, rect, 1.0)
                .into_iter()
                .find(|(action, _)| *action == dacode::source_control::RowAction::Discard)
                .expect("a discard button");
            app.click_source_control(
                sidebar,
                button.x + button.width / 2.0,
                button.y + button.height / 2.0,
                1.0,
            );
        };

        press_discard(&mut app, "notes.txt");
        quiet(&mut app);
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
            "one\nCHANGED\n",
            "one press only asks"
        );
        press_discard(&mut app, "scratch.txt");
        press_discard(&mut app, "notes.txt");
        quiet(&mut app);
        assert!(
            dir.join("scratch.txt").exists(),
            "a press on another file asked again rather than answering"
        );
        press_discard(&mut app, "notes.txt");
        assert!(settle(&mut app, |app| app.state.scm.count(Group::Changes) == 1));
        quiet(&mut app);
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
            "one\ntwo\n"
        );

        press_discard(&mut app, "scratch.txt");
        press_discard(&mut app, "scratch.txt");
        assert!(settle(&mut app, |app| app.state.scm.count(Group::Changes) == 0));
        assert!(!dir.join("scratch.txt").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_commit_made_outside_the_editor_asks_for_the_blame_and_the_gutter_again() {
        // The defect this fixes: a commit or checkout in a terminal changed who wrote every line,
        // and the blame said so only after the file was saved or reopened — the watch on the
        // repository is what notices now.
        let Some(dir) = repository("outside") else {
            return;
        };
        let mut app = editor(&dir, "notes.txt", "one\ntwo\nthree\n");
        // The blame is not wanted, so the flag that asks for one stays where a test can see it.
        app.state.blame.inline = false;
        app.state.blame.column = false;
        app.refresh_scm(false);
        assert!(settle(&mut app, |app| app.state.scm.status().is_some()));
        quiet(&mut app);
        app.blame_wanted = false;

        git(&dir, &["add", "notes.txt"]).expect("staged");
        git(&dir, &["commit", "-q", "-m", "elsewhere"]).expect("committed");

        assert!(
            settle(&mut app, |app| app.blame_wanted),
            "the commit was never noticed"
        );
        assert!(
            settle(&mut app, |app| app.state.scm.count(Group::Changes) == 0),
            "and the list was asked again"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_change_opens_as_a_read_only_diff_tab_that_a_second_click_reuses() {
        let Some(dir) = repository("diff") else {
            return;
        };
        let mut app = editor(&dir, "notes.txt", "one\ntwo\nthree\n");
        app.press(Key::ctrl('g').with_shift());
        assert!(settle(&mut app, |app| app.state.scm.count(Group::Changes) == 1));
        quiet(&mut app);

        app.state.scm.field = Field::List;
        app.state.scm.selected = row_of(&app, Group::Changes, "notes.txt");
        app.press(Key::named(KeyCode::Enter));
        assert!(
            settle(&mut app, |app| app.state.review.is_some()),
            "the diff never opened"
        );
        let text = app.state.document.text().to_string();
        assert!(text.contains("+three"), "{text}");
        assert_eq!(
            app.state.buffer_count, 2,
            "a tab of its own beside the file"
        );
        assert_eq!(
            app.state.path.last().map(String::as_str),
            Some("notes.txt (working tree)")
        );
        assert!(
            !app.state.diff.is_empty(),
            "its hunks are marks, so `]c` walks them as it walks a file's"
        );

        // Read only: typing into it changes nothing, and says why.
        app.state.focus = dacode::focus::Focus::Editor;
        app.press(Key::char('x'));
        assert_eq!(app.state.document.text().to_string(), text);
        assert!(
            app.state
                .hover
                .as_deref()
                .unwrap_or("")
                .contains("not a file")
        );

        // Opening it again goes back to the same tab.
        app.state.focus = dacode::focus::Focus::Sidebar;
        app.state.scm.field = Field::List;
        app.switch_to(0);
        app.press(Key::named(KeyCode::Enter));
        assert!(settle(&mut app, |app| app.state.review.is_some()));
        assert_eq!(app.state.buffer_count, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_git_does_not_track_offers_to_initialize_one() {
        if git(std::path::Path::new("."), &["--version"]).is_none() {
            return;
        }
        let dir = scratch("init");
        let mut app = editor(&dir, "a.txt", "a\n");
        app.press(Key::ctrl('g').with_shift());
        assert!(settle(&mut app, |app| matches!(
            app.state.scm.state,
            dacode::source_control::State::NotARepository
        )));
        let sidebar = app.layout().sidebar.expect("open");
        let geometry = app.state.scm.geometry(sidebar, 1.0, &app.state.typography);
        let button = geometry.init.expect("the button");
        app.click_source_control(sidebar, button.x + 4.0, button.y + 4.0, 1.0);
        assert!(
            settle(&mut app, |app| app.state.scm.status().is_some()),
            "{:?}",
            app.state.scm.state
        );
        assert!(dir.join(".git").is_dir());
        std::fs::remove_dir_all(&dir).ok();
    }
}

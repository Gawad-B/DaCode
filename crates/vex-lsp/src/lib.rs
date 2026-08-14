//! `vex-lsp` — a Language Server Protocol client.
//!
//! Three parts: [`transport`] frames JSON-RPC over a pipe, [`position`] converts between the
//! editor's coordinates and the protocol's, and [`client`] owns the process and the conversation.
//!
//! Two decisions worth stating, both deviations from the original plan:
//!
//! 1. **No async runtime.** The plan named `tokio`. A language server is one child process and two
//!    pipes, which threads and channels model directly — the same shape the syntax worker uses. An
//!    async runtime would be a large dependency serving no need in an editor built to stay light.
//! 2. **Full document sync, not incremental.** Incremental sync sends less, but requires client and
//!    server to agree on every position; one disagreement corrupts the server's copy *silently* and
//!    produces diagnostics for text that does not exist. Full sync is obviously correct, and is an
//!    optimization to revisit with a measurement rather than a guess.
//!
//! The 60fps contract is unchanged: the UI thread sends and polls, never waits, and a result
//! computed against a superseded document version is discarded rather than applied.

pub mod client;
pub mod position;
pub mod transport;

use std::path::PathBuf;

pub use client::{CodeActionItem, Diagnostics, LspClient, Pending, ServerEvent, State};
/// Re-exported so consumers can name severities and ranges without depending on the crate directly.
pub use lsp_types;
pub use position::{from_lsp, path_to_uri, range_from_lsp, range_to_lsp, to_lsp, uri_to_path};

/// Which server to run for a language, and what to call the language on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSpec {
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// The protocol's `languageId`.
    pub language_id: &'static str,
    /// What the language is called in a sentence: "Python", not "py".
    ///
    /// The statusline used to say `pyright-langserver not installed`, which names a program most
    /// people have never heard of, does not say it is a *Python* server, does not say it is a
    /// separate thing from the Python interpreter, and offers no way forward. Someone with Python
    /// installed reads that as "vex cannot see my Python", and they are not being unreasonable.
    pub language: &'static str,
    /// One line that installs it.
    pub install: &'static str,
}

const fn spec(
    program: &'static str,
    args: &'static [&'static str],
    language_id: &'static str,
    language: &'static str,
    install: &'static str,
) -> ServerSpec {
    ServerSpec {
        program,
        args,
        language_id,
        language,
        install,
    }
}

/// The servers vex knows how to start for a file, best first.
///
/// A list rather than one, because "the Python server" is not a single program: pyright is an npm
/// package, pylsp is a pip package, and a given machine has whichever its owner happened to install.
/// Offering one and reporting it missing is how an editor tells someone with a perfectly good
/// language server that it cannot find one.
pub fn servers_for(path: &str) -> &'static [ServerSpec] {
    const RUST: [ServerSpec; 1] = [spec(
        "rust-analyzer",
        &[],
        "rust",
        "Rust",
        "rustup component add rust-analyzer",
    )];
    const TYPESCRIPT: [ServerSpec; 1] = [spec(
        "typescript-language-server",
        &["--stdio"],
        "typescript",
        "TypeScript",
        "npm i -g typescript-language-server typescript",
    )];
    // Three, in preference order. pyright comes from npm, pylsp from pip, and a machine has
    // whichever its owner happened to install — which is the whole reason this is a list.
    const PYTHON: [ServerSpec; 3] = [
        spec(
            "pyright-langserver",
            &["--stdio"],
            "python",
            "Python",
            "npm i -g pyright",
        ),
        spec(
            "basedpyright-langserver",
            &["--stdio"],
            "python",
            "Python",
            "pip install basedpyright",
        ),
        spec(
            "pylsp",
            &[],
            "python",
            "Python",
            "pip install python-lsp-server",
        ),
    ];
    const C_FAMILY: [ServerSpec; 1] = [spec(
        "clangd",
        &[],
        "cpp",
        "C and C++",
        "install LLVM, or `apt install clangd`",
    )];
    const GO: [ServerSpec; 1] = [spec(
        "gopls",
        &[],
        "go",
        "Go",
        "go install golang.org/x/tools/gopls@latest",
    )];
    const LUA: [ServerSpec; 1] = [spec(
        "lua-language-server",
        &[],
        "lua",
        "Lua",
        "install lua-language-server from your package manager",
    )];
    const DART: [ServerSpec; 1] = [spec(
        "dart",
        &["language-server", "--protocol=lsp"],
        "dart",
        "Dart",
        "install the Dart SDK",
    )];

    match path.rsplit('.').next().unwrap_or_default() {
        "rs" => &RUST,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => &TYPESCRIPT,
        "py" | "pyi" => &PYTHON,
        "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "hh" => &C_FAMILY,
        "go" => &GO,
        "lua" => &LUA,
        "dart" => &DART,
        _ => &[],
    }
}

/// The first server vex would try for a file.
///
/// Kept for callers that only want to know whether the language is supported at all.
pub fn server_for(path: &str) -> Option<ServerSpec> {
    servers_for(path).first().copied()
}

/// Whether a program exists on `PATH`.
///
/// Checked before spawning so the editor can say "rust-analyzer is not installed" instead of
/// surfacing a raw `NotFound` from deep in the process machinery.
///
/// Presence is **not** proof the server works: `~/.cargo/bin/rust-analyzer` is a rustup shim that
/// exists whether or not the component is installed, and exits with an explanation when it is not.
/// That is why the client keeps the server's stderr and reports it — this check only avoids the
/// unhelpful failure mode, it does not promise success.
/// Whether a server on `PATH` can actually *start*.
///
/// [`is_installed`] only proves a file exists, and on any rustup machine `rust-analyzer` is a shim that
/// exists whether or not the component is installed — running it prints
/// "Unknown binary 'rust-analyzer' in official toolchain" and exits. That was already recorded as a
/// finding in this project's notes; CI then failed on exactly it, because a test that trusted
/// `is_installed` tried to speak LSP to a shim.
///
/// Spawns `program --version`, which every language server supports and which is fast. Not called on
/// the editor's startup path — it costs a process spawn, and the editor learns the same thing a moment
/// later when the server it launched exits. Used by tests to decide whether there is a server to test
/// against at all.
pub fn can_start(program: &str) -> bool {
    let Some(resolved) = resolve(program) else {
        return false;
    };
    let mut command = std::process::Command::new(resolved);
    client::no_window(&mut command);
    command
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Where a program is on `PATH`, if it is there at all.
///
/// Returns the **path**, not a yes-or-no, and that is the whole point. The old check honoured
/// `PATHEXT` and so found `pyright-langserver.cmd`; the spawn beside it did `Command::new` on the
/// bare name, and Rust's Windows resolver appends only `.exe` — `PATHEXT` is a shell concept it
/// deliberately does not implement. So the two disagreed, and every language server that npm
/// installs as a `.cmd` shim — pyright and typescript-language-server, the two most common servers
/// in the world — passed the check and then failed to spawn. Handing the resolved path to
/// `Command::new` fixes both halves: std detects a `.bat`/`.cmd` and routes it through `cmd.exe`
/// itself, with the argument escaping that CVE-2024-24576 added. Wrapping it in `cmd /c` by hand
/// would reopen exactly that hole.
///
/// Takes its environment as parameters so the Windows rules have tests that run anywhere — the same
/// reason `paths::resolve` and `position::encode_uri` do.
pub fn resolve_in(
    program: &str,
    path: &std::ffi::OsStr,
    pathext: Option<&str>,
    windows: bool,
) -> Option<PathBuf> {
    // Only the extensions that name something executable. A real machine's PATHEXT includes `.JS`,
    // and a stray `pyright-langserver.js` would satisfy the check and then not run.
    const RUNNABLE: [&str; 4] = ["exe", "cmd", "bat", "com"];

    let extensions: Vec<String> = if windows {
        pathext
            .unwrap_or(".EXE;.CMD;.BAT;.COM")
            .split(';')
            .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
            .filter(|extension| RUNNABLE.contains(&extension.as_str()))
            .collect()
    } else {
        Vec::new()
    };

    std::env::split_paths(path).find_map(|directory| {
        let bare = directory.join(program);
        if bare.is_file() && (!windows || bare.extension().is_some()) {
            return Some(bare);
        }
        extensions
            .iter()
            .map(|extension| directory.join(format!("{program}.{extension}")))
            .find(|candidate| candidate.is_file())
    })
}

/// [`resolve_in`] against this process's environment.
pub fn resolve(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").ok();
    resolve_in(program, &path, pathext.as_deref(), cfg!(windows))
}

pub fn is_installed(program: &str) -> bool {
    resolve(program).is_some()
}

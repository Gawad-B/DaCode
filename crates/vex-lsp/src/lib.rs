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
}

/// The servers vex knows how to start, by file extension.
///
/// These are the languages from the plan. A server that is not installed is reported at startup
/// rather than silently skipped, because "no diagnostics" and "no server" look identical otherwise.
pub fn server_for(path: &str) -> Option<ServerSpec> {
    let extension = path.rsplit('.').next()?;
    Some(match extension {
        "rs" => ServerSpec {
            program: "rust-analyzer",
            args: &[],
            language_id: "rust",
        },
        "ts" | "tsx" | "js" | "jsx" => ServerSpec {
            program: "typescript-language-server",
            args: &["--stdio"],
            language_id: "typescript",
        },
        "py" => ServerSpec {
            program: "pyright-langserver",
            args: &["--stdio"],
            language_id: "python",
        },
        "c" | "h" | "cpp" | "hpp" | "cc" => ServerSpec {
            program: "clangd",
            args: &[],
            language_id: "cpp",
        },
        "dart" => ServerSpec {
            program: "dart",
            args: &["language-server", "--protocol=lsp"],
            language_id: "dart",
        },
        _ => return None,
    })
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
pub fn is_installed(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}

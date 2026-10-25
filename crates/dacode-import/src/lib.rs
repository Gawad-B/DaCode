//! `dacode-import` — bring settings, keybindings, themes and snippets over from VSCode.
//!
//! # What this can and cannot do
//!
//! **VSCode extensions do not run in DaCode, and cannot be made to.** Two independent reasons:
//!
//! 1. *Legal.* Microsoft's marketplace terms limit use to "in-scope products" — Visual Studio,
//!    VS Code, Codespaces and Azure DevOps. Non-Microsoft editors are not permitted to fetch from
//!    it. [Open VSX](https://open-vsx.org) is the legally redistributable, API-compatible registry
//!    (the one VSCodium and Gitpod use), so that is what this module targets.
//! 2. *Architectural.* Extensions are JavaScript executing against the `vscode` API inside Node.
//!    Supporting them means embedding a JS runtime and reimplementing that API surface, plus a
//!    Node-sized process per extension. That is precisely what makes VSCode heavy, and it
//!    contradicts this editor's reason for existing.
//!
//! What *is* portable is the part of an extension that is **data** — themes, snippets, language
//! configuration — plus the part that is **a language server**, which DaCode can talk to over LSP
//! directly rather than through an extension host. That covers most of the practical value: the
//! high-value extensions (rust-analyzer, Pyright, clangd, gopls) are largely thin wrappers around
//! a server binary.
//!
//! # Nothing is imported unless it is asked for
//!
//! Every import goes through a [`Checklist`]: the caller enumerates what a source *offers*, then
//! selects what to take. An importer that quietly rewrote your configuration would be worse than
//! no importer, so the default is to take nothing.

pub mod keys;
pub mod palette;
pub mod plan;
pub mod settings;
pub mod snippets;
pub mod theme;
pub mod vsix;

pub use keys::{ImportedKeybinding, import_keybindings};
pub use palette::{Format, read as read_palette};
pub use plan::{Checklist, ImportItem, ImportReport, Outcome};
pub use settings::{ImportedSettings, import_settings};
pub use snippets::{Snippet, import_snippets};
pub use theme::import_theme;
pub use vsix::{Capability, Extension, read_vsix};

use anyhow::{Context, Result};

/// Strips comments and trailing commas so `serde_json` can read VSCode's JSON-with-comments.
///
/// VSCode's own config files are JSONC: `settings.json` ships with explanatory comments and
/// trailing commas are common. Rejecting those would make the importer fail on almost every real
/// configuration, so this normalizes first. String contents and escapes are respected — a `//`
/// inside a string is not a comment.
pub fn parse_jsonc(source: &str) -> Result<serde_json::Value> {
    let cleaned = strip_jsonc(source);
    serde_json::from_str(&cleaned).context("the file is not valid JSON, even allowing for comments")
}

fn strip_jsonc(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    // Keep newlines so error positions still line up with the source.
                    if c == '\n' {
                        out.push('\n');
                    }
                    previous = c;
                }
            }
            _ => out.push(c),
        }
    }

    remove_trailing_commas(&out)
}

fn remove_trailing_commas(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = source.chars().collect();

    for (index, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }

        if c == ',' {
            // A comma is trailing if the next non-whitespace character closes the container.
            let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(c);
    }

    out
}

/// Where VSCode keeps its user configuration on this platform.
///
/// Returned whether or not it exists, so callers can report a precise "looked here" message
/// rather than a vague failure.
pub fn vscode_config_dir() -> Option<std::path::PathBuf> {
    // Windows keeps it under the roaming profile; every other platform under the home directory.
    // Checked before `HOME` because a POSIX shell on Windows sets both, and `%APPDATA%` is where
    // VSCode itself writes.
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let appdata = std::path::PathBuf::from(appdata);
            for candidate in ["Code/User", "VSCodium/User", "Code - OSS/User"] {
                let path = appdata.join(candidate);
                if path.join("settings.json").exists() || path.join("snippets").exists() {
                    return Some(path);
                }
            }
        }
    }

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)?;

    // Linux; the same layout with different roots applies to macOS and Windows, which are not v1
    // targets. VSCodium keeps an identical structure under a different directory name.
    for candidate in ["Code/User", "VSCodium/User", "Code - OSS/User"] {
        let path = home.join(".config").join(candidate);
        if path.exists() {
            return Some(path);
        }
    }
    Some(home.join(".config/Code/User"))
}

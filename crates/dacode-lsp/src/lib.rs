//! `dacode-lsp` — a Language Server Protocol client.
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

use std::path::{Path, PathBuf};

pub use client::{
    CodeActionItem, CompletionItem, Diagnostics, LspClient, Pending, ServerEvent, Signature, State,
    Symbol, symbols_from,
};
/// Re-exported so consumers can name severities and ranges without depending on the crate directly.
pub use lsp_types;
pub use position::{from_lsp, path_to_uri, range_from_lsp, range_to_lsp, to_lsp, uri_to_path};

/// Which server to run for a language, and what to call the language on the wire.
///
/// Owned rather than `&'static str`, which it used to be, because a server named in `init.lua` is
/// not static and there is no useful way to pretend otherwise. The cost is an allocation per
/// lookup, paid when a buffer is opened rather than per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    pub program: String,
    pub args: Vec<String>,
    /// The protocol's `languageId`.
    pub language_id: String,
    /// What the language is called in a sentence: "Python", not "py".
    ///
    /// The statusline used to say `pyright-langserver not installed`, which names a program most
    /// people have never heard of, does not say it is a *Python* server, does not say it is a
    /// separate thing from the Python interpreter, and offers no way forward. Someone with Python
    /// installed reads that as "DaCode cannot see my Python", and they are not being unreasonable.
    pub language: String,
    /// One line that installs it, or empty for a server `init.lua` named — nobody can be told how
    /// to install a program only they know about, and the useful message there is a different one.
    pub install: String,
    /// Commands that would install it, best first — empty where none is honest.
    ///
    /// A *list*, because "how do I install this" has more than one right answer and which one is
    /// right depends on the machine. `pip install` is upstream's own fallback for the Python
    /// servers rather than its recommendation: on Debian, Ubuntu, Fedora, Arch and Homebrew Python
    /// it refuses outright under PEP 668, and `uv` or `pipx` is what actually works. Picking one at
    /// build time would mean choosing which platform the button is broken on.
    ///
    /// Empty for clangd, lua-language-server and the Dart SDK: those come from a system package
    /// manager whose name differs per distribution and which wants root, and DaCode must not run a
    /// package manager as root. Telling someone what to type is the whole answer there — an editor
    /// offering a button that cannot work is worse than one that does not offer it.
    pub installers: Vec<Installer>,
    /// Whether `init.lua` asked for this one rather than DaCode knowing about it.
    pub configured: bool,
}

/// A command that installs a language server, as argv rather than a shell line.
///
/// Never a shell string. These are run through `Command`, which does no word splitting, so a shell
/// line would have to be re-split here — and splitting a string into arguments correctly is a
/// problem nobody should solve twice. The `needs` program is what has to be on PATH first: saying
/// "rustup is not installed" is a better answer than a spawn failure nobody can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installer {
    pub program: String,
    pub args: Vec<String>,
    /// What has to exist before this can run. Usually the same as `program`.
    pub needs: String,
}

/// An installer's `program` and `needs` when the installer is DaCode itself — `dacode tools install`,
/// which downloads a pinned, checksummed release and needs nothing else on the machine. A sentinel
/// rather than `"dacode"`: the editor running is the one to ask, and on Windows it is usually not on
/// `PATH` at all. `dacode::install` resolves it to the running executable.
pub const SELF: &str = "@dacode";

impl Installer {
    /// The command as one line, for showing before it is run. Nothing is ever *parsed* back out of
    /// this — it exists so a person can read what they are about to agree to.
    pub fn line(&self) -> String {
        let program = if self.program == SELF {
            "dacode"
        } else {
            &self.program
        };
        if self.args.is_empty() {
            program.to_string()
        } else {
            format!("{program} {}", self.args.join(" "))
        }
    }

    /// Whether this machine has what the installer needs.
    pub fn runnable(&self) -> bool {
        self.needs == SELF || resolve(&self.needs).is_some()
    }
}

impl ServerSpec {
    /// The best install command this machine can actually run.
    ///
    /// Asks the filesystem, because that is the only place the answer is: a list that put `pip`
    /// first would be right on Windows and refused on most Linux, and one that put `uv` first would
    /// name a program most people do not have. Cheap, and only reached when someone asks to install
    /// something.
    pub fn installer(&self) -> Option<&Installer> {
        self.installers
            .iter()
            .find(|installer| installer.runnable())
    }
}

/// One server DaCode ships knowledge of: a program, its arguments, and the line that installs it.
#[derive(Debug, Clone, Copy)]
struct Builtin {
    program: &'static str,
    args: &'static [&'static str],
    install: &'static str,
    /// The install lines as argv, best first: `(needs, program, args)`.
    runs: &'static [(&'static str, &'static str, &'static [&'static str])],
}

/// A server whose install line is advice only — no one command is right on every platform.
const fn builtin(
    program: &'static str,
    args: &'static [&'static str],
    install: &'static str,
) -> Builtin {
    Builtin {
        program,
        args,
        install,
        runs: &[],
    }
}

/// A server the editor can install itself, given one of `runs`' toolchains on PATH.
///
/// The displayed line is derived from the argv rather than written twice, so what is shown and what
/// is run cannot disagree — which is the whole point of showing it before running it.
const fn installable(
    program: &'static str,
    args: &'static [&'static str],
    runs: &'static [(&'static str, &'static str, &'static [&'static str])],
) -> Builtin {
    Builtin {
        program,
        args,
        install: "",
        runs,
    }
}

impl Builtin {
    fn spec(&self, language: &Language) -> ServerSpec {
        let installers: Vec<Installer> = self
            .runs
            .iter()
            .map(|(needs, program, args)| Installer {
                program: (*program).to_string(),
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                needs: (*needs).to_string(),
            })
            .collect();

        ServerSpec {
            program: self.program.to_string(),
            args: self.args.iter().map(|arg| (*arg).to_string()).collect(),
            language_id: language.id.to_string(),
            language: language.name.to_string(),
            // Derived from the argv where there is one, so the sentence the user reads and the
            // command that runs are the same thing rather than two strings kept in step by hand.
            // Every alternative is named: someone on Debian needs to see `pipx`, not the `pip` line
            // their machine refuses.
            install: if installers.is_empty() {
                self.install.to_string()
            } else {
                installers
                    .iter()
                    .map(Installer::line)
                    .collect::<Vec<_>>()
                    .join(", or ")
            },
            installers,
            configured: false,
        }
    }
}

/// A language DaCode knows about: what its files are called, what the protocol calls it, and what can
/// serve it.
///
/// The table is pivoted around the **language** rather than the program because that is what config
/// keys off. `dacode.server("python", …)` names a language, and matching that to a file needs the
/// extension list — which used to exist only as arms of a `match`, reachable by nothing.
#[derive(Debug, Clone, Copy)]
pub struct Language {
    /// What it is called in a sentence: "Python", not "py".
    pub name: &'static str,
    /// The protocol's `languageId`, and the name `dacode.server` uses.
    pub id: &'static str,
    /// Without the dot.
    pub extensions: &'static [&'static str],
    /// Other names `dacode.server` accepts for this language.
    ///
    /// One entry can serve several languages — `typescript-language-server` serves JavaScript too,
    /// and clangd serves both C and C++ — and the person writing `dacode.server("javascript", …)` is
    /// naming their language, not DaCode's row for it. Without this that call failed with "unknown
    /// language", which reads as "unsupported".
    aliases: &'static [&'static str],
    servers: &'static [Builtin],
    /// Whether a file of this language is whole without its server. HTML and CSS are: Emmet adds
    /// abbreviations and nothing else, so its absence is said on the statusline rather than in a
    /// card over the file — which would be telling everyone who opens a stylesheet that something
    /// is missing from it.
    pub optional: bool,
}

impl Language {
    /// Every server DaCode would try for this language, best first.
    pub fn specs(&self) -> Vec<ServerSpec> {
        self.servers
            .iter()
            .map(|server| server.spec(self))
            .collect()
    }

    /// The programs DaCode would look for, for a message that has to say what it tried.
    pub fn programs(&self) -> Vec<&'static str> {
        self.servers.iter().map(|server| server.program).collect()
    }

    /// The line that installs the first choice, whether or not DaCode can run it.
    pub fn install(&self) -> String {
        self.servers
            .first()
            .map(|server| server.spec(self).install)
            .unwrap_or_default()
    }
}

/// Every language DaCode knows a server for.
///
/// More than one server per language where there genuinely is more than one: "the Python server" is
/// not a single program — pyright comes from npm and pylsp from pip — and a machine has whichever
/// its owner happened to install. Offering only the first and reporting it missing is how an editor
/// tells someone with a perfectly good language server that it cannot find one.
pub const LANGUAGES: [Language; 10] = [
    Language {
        name: "Rust",
        id: "rust",
        aliases: &[],
        extensions: &["rs"],
        optional: false,
        servers: &[installable(
            "rust-analyzer",
            &[],
            &[
                (SELF, SELF, &["tools", "install", "rust-analyzer"]),
                ("rustup", "rustup", &["component", "add", "rust-analyzer"]),
            ],
        )],
    },
    // JavaScript and TypeScript are two entries running one program, which is not a duplication:
    // the entry is what decides the `languageId` in `didOpen`, and that string is how a server is
    // told which language it has been handed. One row called "TypeScript" announced every `.js`
    // file as TypeScript, so the server applied TypeScript's rules to JavaScript — which from the
    // outside looks exactly like "the TypeScript server does not work with .js files". It also
    // left the settings panel with no JavaScript row at all.
    Language {
        name: "JavaScript",
        id: "javascript",
        aliases: &["js", "node"],
        extensions: &["js", "jsx", "mjs", "cjs"],
        optional: false,
        servers: &[installable(
            "typescript-language-server",
            &["--stdio"],
            &[
                (
                    SELF,
                    SELF,
                    &["tools", "install", "typescript-language-server", "prettier"],
                ),
                (
                    "npm",
                    "npm",
                    &[
                        "install",
                        "--global",
                        "typescript-language-server",
                        "typescript",
                    ],
                ),
            ],
        )],
    },
    Language {
        name: "TypeScript",
        id: "typescript",
        aliases: &["ts"],
        extensions: &["ts", "tsx"],
        optional: false,
        servers: &[installable(
            "typescript-language-server",
            &["--stdio"],
            &[
                (
                    SELF,
                    SELF,
                    &["tools", "install", "typescript-language-server", "prettier"],
                ),
                (
                    "npm",
                    "npm",
                    &[
                        "install",
                        "--global",
                        "typescript-language-server",
                        "typescript",
                    ],
                ),
            ],
        )],
    },
    Language {
        name: "Python",
        id: "python",
        aliases: &[],
        extensions: &["py", "pyi"],
        optional: false,
        servers: &[
            installable(
                "pyright-langserver",
                &["--stdio"],
                &[
                    (SELF, SELF, &["tools", "install", "pyright", "ruff"]),
                    ("npm", "npm", &["install", "--global", "pyright"]),
                ],
            ),
            installable(
                "basedpyright-langserver",
                &["--stdio"],
                &[
                    ("uv", "uv", &["tool", "install", "basedpyright"]),
                    ("pipx", "pipx", &["install", "basedpyright"]),
                    ("pip", "pip", &["install", "basedpyright"]),
                ],
            ),
            installable(
                "pylsp",
                &[],
                &[
                    ("uv", "uv", &["tool", "install", "python-lsp-server"]),
                    ("pipx", "pipx", &["install", "python-lsp-server[all]"]),
                    ("pip", "pip", &["install", "python-lsp-server"]),
                ],
            ),
        ],
    },
    Language {
        name: "C and C++",
        id: "cpp",
        aliases: &["c", "c++"],
        extensions: &["c", "h", "cpp", "hpp", "cc", "cxx", "hh"],
        // Downloaded, where clangd publishes a build; elsewhere (ARM64 Linux) the download says so
        // and that it will find one installed from the system's packages.
        optional: false,
        servers: &[installable(
            "clangd",
            &[],
            &[(SELF, SELF, &["tools", "install", "clangd"])],
        )],
    },
    Language {
        name: "Go",
        id: "go",
        aliases: &[],
        extensions: &["go"],
        optional: false,
        servers: &[installable(
            "gopls",
            &[],
            &[("go", "go", &["install", "golang.org/x/tools/gopls@latest"])],
        )],
    },
    Language {
        name: "Lua",
        id: "lua",
        aliases: &[],
        extensions: &["lua"],
        optional: false,
        servers: &[installable(
            "lua-language-server",
            &[],
            &[(
                SELF,
                SELF,
                &["tools", "install", "lua-language-server", "stylua"],
            )],
        )],
    },
    // HTML and CSS are served by Emmet alone, which is what most people open those files in VSCode
    // for: `ul>li*3` and `m10` expanded from the completion list. DaCode runs one server per file,
    // so this is Emmet *instead of* vscode-html-language-server rather than beside it, and Emmet
    // in JSX waits for a second server per buffer.
    Language {
        name: "HTML",
        id: "html",
        aliases: &["htm"],
        extensions: &["html", "htm"],
        optional: true,
        servers: &[installable(
            "emmet-language-server",
            &["--stdio"],
            &[
                (SELF, SELF, &["tools", "install", "emmet-language-server"]),
                (
                    "npm",
                    "npm",
                    &["install", "--global", "@olrtg/emmet-language-server"],
                ),
            ],
        )],
    },
    Language {
        name: "CSS",
        id: "css",
        aliases: &["scss", "less"],
        extensions: &["css", "scss", "less"],
        optional: true,
        servers: &[installable(
            "emmet-language-server",
            &["--stdio"],
            &[
                (SELF, SELF, &["tools", "install", "emmet-language-server"]),
                (
                    "npm",
                    "npm",
                    &["install", "--global", "@olrtg/emmet-language-server"],
                ),
            ],
        )],
    },
    Language {
        name: "Dart",
        id: "dart",
        aliases: &[],
        extensions: &["dart"],
        optional: false,
        servers: &[builtin(
            "dart",
            &["language-server", "--protocol=lsp"],
            "install the Dart SDK",
        )],
    },
];

/// A file's extension, without the dot, or empty when it has none.
///
/// The last component only, and a leading dot does not count: a file named `.gitignore` has no
/// extension, and the old `rsplit('.')` said its extension was `gitignore`. Harmless while every
/// comparison was against a fixed list, and not harmless once config can name extensions.
fn extension_of(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    }
}

/// The language a file is written in, as far as DaCode knows.
pub fn language_for(path: &str) -> Option<&'static Language> {
    let extension = extension_of(path);
    if extension.is_empty() {
        return None;
    }
    LANGUAGES
        .iter()
        .find(|language| language.extensions.contains(&extension))
}

/// A language by the name `dacode.server` uses.
pub fn language_named(id: &str) -> Option<&'static Language> {
    let id = id.trim().to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|language| language.id == id || language.aliases.contains(&id.as_str()))
}

/// A server `init.lua` asked for, and the files it serves.
///
/// Kept beside the spec rather than inside it because the extension list is how a *file* reaches
/// this entry, and the built-in table already answers that question from [`Language`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    pub spec: ServerSpec,
    /// Without the dot.
    pub extensions: Vec<String>,
}

/// Turns what config said into a spec, or explains what is wrong with it.
///
/// The language name is looked up in DaCode's own table, so `dacode.server("python", …)` inherits both the
/// protocol's `languageId` and the extension list without the user restating them. A language DaCode has
/// never heard of is allowed — it simply has to say which files it serves, because a server that can
/// never match a file is a line of config that silently does nothing, which is the worst outcome
/// available.
pub fn configure(
    language: &str,
    command: &str,
    args: &[String],
    extensions: &[String],
) -> Result<Configured, String> {
    let known = language_named(language);
    let command = command.trim();
    if command.is_empty() {
        return Err(format!(
            "dacode.server(\"{language}\", …) needs a command — the program that speaks LSP"
        ));
    }

    let extensions: Vec<String> = if extensions.is_empty() {
        match known {
            Some(known) => known.extensions.iter().map(|e| (*e).to_string()).collect(),
            None => {
                return Err(format!(
                    "dacode.server(\"{language}\", …): DaCode does not know a language called \
                     `{language}`, so say which files it serves — \
                     extensions = {{ \"{language}\" }}"
                ));
            }
        }
    } else {
        extensions
            .iter()
            .map(|extension| {
                extension
                    .trim()
                    .trim_start_matches('.')
                    .to_ascii_lowercase()
            })
            .filter(|extension| !extension.is_empty())
            .collect()
    };

    if extensions.is_empty() {
        return Err(format!(
            "dacode.server(\"{language}\", …): the extensions list is empty, so nothing would ever \
             use it"
        ));
    }

    Ok(Configured {
        spec: ServerSpec {
            program: command.to_string(),
            args: args.to_vec(),
            // An unknown language's id is what the user typed, which is what a server for it will
            // expect: they are the only two parties who know the name.
            language_id: known.map(|k| k.id).unwrap_or(language).to_string(),
            language: known.map(|k| k.name).unwrap_or(language).to_string(),
            install: String::new(),
            // Nobody can be told how to install a program only they know about, and offering to run
            // something for a command the config named would be guessing with a package manager.
            installers: Vec::new(),
            configured: true,
        },
        extensions,
    })
}

/// The servers DaCode would try for a file, best first: what config asked for, then what DaCode knows.
///
/// Configured entries go **first** rather than replacing the built-ins, so naming a server for a
/// venv does not also throw away the fallback when that venv is not there. The statusline names
/// whichever one actually started, so the order is visible rather than a guess.
pub fn servers_for(path: &str, configured: &[Configured]) -> Vec<ServerSpec> {
    let extension = extension_of(path);
    if extension.is_empty() {
        return Vec::new();
    }

    let mut specs: Vec<ServerSpec> = configured
        .iter()
        .filter(|entry| entry.extensions.iter().any(|known| known == extension))
        .map(|entry| entry.spec.clone())
        .collect();

    if let Some(language) = language_for(path) {
        specs.extend(language.specs());
    }
    specs
}

/// The first server DaCode would try for a file.
///
/// Kept for callers that only want to know whether the language is supported at all.
pub fn server_for(path: &str, configured: &[Configured]) -> Option<ServerSpec> {
    servers_for(path, configured).into_iter().next()
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
    let extensions = runnable_extensions(pathext, windows);

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

/// The `PATHEXT` entries that name something actually runnable.
///
/// Filtered rather than taken whole: a real machine's `PATHEXT` includes `.JS`, and a stray
/// `pyright-langserver.js` would satisfy the check and then not run.
fn runnable_extensions(pathext: Option<&str>, windows: bool) -> Vec<String> {
    const RUNNABLE: [&str; 4] = ["exe", "cmd", "bat", "com"];

    if !windows {
        return Vec::new();
    }
    pathext
        .unwrap_or(".EXE;.CMD;.BAT;.COM")
        .split(';')
        .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
        .filter(|extension| RUNNABLE.contains(&extension.as_str()))
        .collect()
}

/// A path as written, or the same path with an executable extension Windows would have added.
fn executable_at(base: PathBuf, extensions: &[String]) -> Option<PathBuf> {
    if base.is_file() {
        return Some(base);
    }
    // `.venv/bin/pylsp` is what someone writes on any platform; on Windows the file is
    // `pylsp.exe`, and refusing the path they wrote because of a suffix would be pedantry with a
    // broken editor attached.
    extensions.iter().find_map(|extension| {
        let mut candidate = base.clone().into_os_string();
        candidate.push(".");
        candidate.push(extension);
        let candidate = PathBuf::from(candidate);
        candidate.is_file().then_some(candidate)
    })
}

/// Where a program named by config is: a `PATH` lookup, or a path.
///
/// **A command containing a separator is a path, not a name.** That is the whole reason this exists
/// beside [`resolve_in`]: `command = ".venv/bin/pylsp"` names one project's interpreter's server,
/// and searching `PATH` for a string containing a slash finds nothing, ever. A leading `~/` is
/// expanded for the same reason — it is what people write, and silently not finding it is exactly
/// the failure this feature was added to end.
///
/// A relative path is resolved against the **project root** first, not the process's working
/// directory: for a windowed editor that directory is wherever the desktop happened to launch it
/// from, and never what the user meant by `.venv`.
///
/// Takes its environment as parameters for the reason every platform-shaped function here does —
/// the Windows rules then have tests that run on Linux.
pub fn resolve_command_in(
    program: &str,
    root: &Path,
    home: Option<&Path>,
    path: &std::ffi::OsStr,
    pathext: Option<&str>,
    windows: bool,
) -> Option<PathBuf> {
    let is_path = program.contains('/') || (windows && program.contains('\\'));
    if !is_path && !program.starts_with('~') {
        return resolve_in(program, path, pathext, windows);
    }

    let extensions = runnable_extensions(pathext, windows);
    if let Some(rest) = program
        .strip_prefix("~/")
        .or_else(|| program.strip_prefix("~\\"))
    {
        return executable_at(home?.join(rest), &extensions);
    }

    let direct = PathBuf::from(program);
    if direct.is_absolute() {
        return executable_at(direct, &extensions);
    }
    executable_at(root.join(program), &extensions).or_else(|| executable_at(direct, &extensions))
}

/// [`resolve_in`] against this process's environment.
pub fn resolve(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").ok();
    resolve_in(program, &path, pathext.as_deref(), cfg!(windows))
}

/// [`resolve_command_in`] against this process's environment.
pub fn resolve_command(program: &str, root: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").ok();
    resolve_command_in(
        program,
        root,
        home,
        &path,
        pathext.as_deref(),
        cfg!(windows),
    )
}

pub fn is_installed(program: &str) -> bool {
    resolve(program).is_some()
}

#[cfg(test)]
mod language_name_tests {
    use super::*;

    /// A JavaScript file is announced to the server as JavaScript.
    ///
    /// This is the whole of why the two are separate entries rather than one row named for both.
    /// The entry decides the `languageId` sent in `didOpen`, so a single row called "TypeScript"
    /// told the server that every `.js` file was TypeScript — and a server told that applies
    /// TypeScript's rules to it. Reported as "the TypeScript server does not work with .js files",
    /// which is precisely what it looks like from the outside. The row's *name* is also what the
    /// settings panel lists, so there was no JavaScript in it to find.
    #[test]
    fn a_javascript_file_is_announced_as_javascript() {
        let language = language_for("app.js").expect("a .js file has a language");
        assert_eq!(language.id, "javascript");
        assert_eq!(language.name, "JavaScript");

        for name in ["component.jsx", "module.mjs", "config.cjs"] {
            assert_eq!(
                language_for(name).map(|language| language.id),
                Some("javascript"),
                "{name} is JavaScript"
            );
        }
        for name in ["app.ts", "component.tsx"] {
            assert_eq!(
                language_for(name).map(|language| language.id),
                Some("typescript"),
                "{name} is TypeScript"
            );
        }
    }

    /// Both run the same program, which is what makes separating them free.
    #[test]
    fn javascript_and_typescript_are_served_by_the_same_program() {
        let js = language_named("javascript").expect("javascript is in the table");
        let ts = language_named("typescript").expect("so is typescript");
        assert_eq!(js.programs(), ts.programs());
        assert_eq!(js.install(), ts.install());
    }

    /// `dacode.server("js", …)` names a language, not DaCode's row for it.
    #[test]
    fn a_language_can_be_configured_by_the_name_its_users_call_it() {
        assert_eq!(
            language_named("js").map(|language| language.id),
            Some("javascript")
        );
        assert_eq!(
            language_named("c").map(|language| language.id),
            Some("cpp"),
            "clangd serves C, and somebody configuring it will write \"c\""
        );
    }

    /// Every name must belong to one entry, or which one wins is decided by table order.
    #[test]
    fn no_two_languages_answer_to_the_same_name() {
        let mut seen = std::collections::HashSet::new();
        for language in &LANGUAGES {
            assert!(seen.insert(language.id), "duplicate id {}", language.id);
            for alias in language.aliases {
                assert!(
                    seen.insert(alias),
                    "{alias} names more than one language, so language_named picks by table order"
                );
            }
        }
    }

    /// Every alias must reach an entry that actually serves files, not a name that resolves to
    /// nothing — the whole point is that configuring by it works.
    #[test]
    fn every_alias_reaches_a_language_that_serves_files() {
        for language in &LANGUAGES {
            for alias in language.aliases {
                let found = language_named(alias).expect("an alias resolves");
                assert!(
                    !found.extensions.is_empty(),
                    "{alias} resolves to an entry that serves no file"
                );
            }
        }
    }
}

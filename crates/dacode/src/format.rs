//! Formatting a buffer with an external program.
//!
//! `format_on_save` and `gf` went through the language server and nowhere else, which is fine for
//! Rust and Go — rust-analyzer runs rustfmt, gopls runs gofmt — and useless for the languages most
//! people write. There is no language server in a plain JavaScript, CSS or Markdown project, and
//! pyright, the most common Python server, does not format at all. So the one feature everybody
//! expects an editor to have was missing exactly where it was most expected.
//!
//! Three rules, all of them the same rule this codebase already has:
//!
//! **It runs on a worker.** A formatter is a process spawn, and some of them take a second on a
//! large file. Waiting for one on the UI thread would freeze the editor mid-save.
//!
//! **The result is tagged with the version it saw.** Text formatted against a document the user has
//! since typed into is discarded, not merged. The same contract as syntax, git and LSP.
//!
//! **Text goes in on stdin and comes back on stdout.** Never a path: a formatter given the file
//! would rewrite it on disk behind the editor, and the buffer — which may be dirty — would then
//! disagree with it. Every formatter worth having supports this; the table only holds ones that do.

use crate::wake::Waker;
use dacode_text::Version;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// A program that reads source on stdin and writes it back on stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatter {
    pub program: String,
    pub args: Vec<String>,
    /// What the language is called in a sentence, for the message when it is missing.
    pub language: String,
    /// One line that installs it, or empty for one `init.lua` named.
    pub install: String,
    pub configured: bool,
}

/// One formatter DaCode ships knowledge of.
#[derive(Debug, Clone, Copy)]
struct Builtin {
    program: &'static str,
    args: &'static [&'static str],
    install: &'static str,
}

const fn builtin(
    program: &'static str,
    args: &'static [&'static str],
    install: &'static str,
) -> Builtin {
    Builtin {
        program,
        args,
        install,
    }
}

/// A language DaCode knows how to format, and what it would use.
struct Known {
    /// The `dacode_lsp::Language` id where there is one, so `dacode.formatter("python", …)` and
    /// `dacode.server("python", …)` name the same thing.
    id: &'static str,
    name: &'static str,
    extensions: &'static [&'static str],
    /// Best first, as with servers: a machine has whichever its owner installed.
    programs: &'static [Builtin],
}

/// What DaCode will try, per language.
///
/// Every entry reads stdin and writes stdout — `prettier` needs `--stdin-filepath` to know which
/// parser to use, `black` and `ruff` need a literal `-`, and `clang-format` wants `--assume-filename`
/// for the same reason prettier does. A formatter that only rewrites files in place is deliberately
/// absent: it would edit the file behind a dirty buffer.
const KNOWN: [Known; 8] = [
    Known {
        id: "rust",
        name: "Rust",
        extensions: &["rs"],
        programs: &[builtin(
            "rustfmt",
            &["--emit", "stdout", "--edition", "2021"],
            "rustup component add rustfmt",
        )],
    },
    Known {
        id: "typescript",
        name: "JavaScript and TypeScript",
        extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        programs: &[
            builtin("prettier", &["--stdin-filepath"], "npm i -g prettier"),
            builtin(
                "biome",
                &["format", "--stdin-file-path"],
                "npm i -g @biomejs/biome",
            ),
        ],
    },
    Known {
        id: "python",
        name: "Python",
        extensions: &["py", "pyi"],
        programs: &[
            builtin("ruff", &["format", "-"], "pip install ruff"),
            builtin("black", &["-", "-q"], "pip install black"),
        ],
    },
    Known {
        id: "cpp",
        name: "C and C++",
        extensions: &["c", "h", "cpp", "hpp", "cc", "cxx", "hh"],
        programs: &[builtin(
            "clang-format",
            &["--assume-filename"],
            "install LLVM, or `apt install clang-format`",
        )],
    },
    Known {
        id: "go",
        name: "Go",
        extensions: &["go"],
        programs: &[builtin("gofmt", &[], "comes with the Go SDK")],
    },
    Known {
        id: "json",
        name: "JSON",
        extensions: &["json", "jsonc"],
        programs: &[builtin(
            "prettier",
            &["--stdin-filepath"],
            "npm i -g prettier",
        )],
    },
    Known {
        id: "css",
        name: "CSS and HTML",
        extensions: &["css", "scss", "less", "html", "vue", "svelte"],
        programs: &[builtin(
            "prettier",
            &["--stdin-filepath"],
            "npm i -g prettier",
        )],
    },
    Known {
        id: "markdown",
        name: "Markdown and YAML",
        extensions: &["md", "markdown", "yaml", "yml"],
        programs: &[builtin(
            "prettier",
            &["--stdin-filepath"],
            "npm i -g prettier",
        )],
    },
];

/// The languages `dacode.formatter` accepts, for a did-you-mean on a typo.
pub fn language_names() -> Vec<&'static str> {
    KNOWN.iter().map(|known| known.id).collect()
}

/// A formatter named by `init.lua`, and the files it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    pub formatter: Formatter,
    /// Without the dot.
    pub extensions: Vec<String>,
}

/// Turns what config said into a formatter, or explains what is wrong with it.
///
/// The same shape as `dacode_lsp::configure`, and for the same reasons: an unknown language is allowed
/// but has to say which files it serves, because a formatter that can never match a file is a line
/// of config that silently does nothing.
pub fn configure(
    language: &str,
    command: &str,
    args: &[String],
    extensions: &[String],
) -> Result<Configured, String> {
    let known = KNOWN.iter().find(|known| known.id == language.trim());
    let command = command.trim();
    if command.is_empty() {
        return Err(format!(
            "dacode.formatter(\"{language}\", …) needs a command — the program that formats"
        ));
    }

    let extensions: Vec<String> = if extensions.is_empty() {
        match known {
            Some(known) => known.extensions.iter().map(|e| (*e).to_string()).collect(),
            None => {
                return Err(format!(
                    "dacode.formatter(\"{language}\", …): DaCode does not know a language called \
                     `{language}`, so say which files it formats — \
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
            "dacode.formatter(\"{language}\", …): the extensions list is empty, so nothing would ever \
             use it"
        ));
    }

    Ok(Configured {
        formatter: Formatter {
            program: command.to_string(),
            args: args.to_vec(),
            language: known.map(|k| k.name).unwrap_or(language).to_string(),
            install: String::new(),
            configured: true,
        },
        extensions,
    })
}

/// A file's extension, without the dot. The same rule `dacode_lsp` uses: a dotfile has none.
fn extension_of(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    }
}

/// The formatters DaCode would try for a file, best first: config's, then its own.
///
/// Configured entries go first and the built-ins stay behind them, the same merge as language
/// servers — naming `ruff` should not cost you the `black` fallback on a machine that has only that.
pub fn formatters_for(path: &str, configured: &[Configured]) -> Vec<Formatter> {
    let extension = extension_of(path);
    if extension.is_empty() {
        return Vec::new();
    }

    let mut all: Vec<Formatter> = configured
        .iter()
        .filter(|entry| entry.extensions.iter().any(|known| known == extension))
        .map(|entry| entry.formatter.clone())
        .collect();

    if let Some(known) = KNOWN
        .iter()
        .find(|known| known.extensions.contains(&extension))
    {
        all.extend(known.programs.iter().map(|program| Formatter {
            program: program.program.to_string(),
            args: program.args.iter().map(|arg| (*arg).to_string()).collect(),
            language: known.name.to_string(),
            install: program.install.to_string(),
            configured: false,
        }));
    }
    all
}

/// A request to format a snapshot of a buffer.
pub struct Request {
    pub formatter: Formatter,
    /// Where the formatter is, already resolved — the worker does no `PATH` lookup, so the same
    /// venv and project-root rules that apply to a language server apply here.
    pub executable: PathBuf,
    /// The file being formatted, for the formatters that need to be told which parser to use.
    pub path: PathBuf,
    pub root: PathBuf,
    pub text: String,
    pub version: Version,
}

/// What came back.
pub struct Formatted {
    pub version: Version,
    /// The formatted text, or nothing when the program failed.
    pub text: Option<String>,
    /// What went wrong, in the user's terms.
    pub problem: Option<String>,
}

/// A background thread that runs formatters.
///
/// One thread rather than one per request, for the reason the git worker gives: a formatter is a
/// process spawn, and a fan-out of them is exactly the unbounded work the one rule forbids. Unlike
/// the git worker this does **not** drop queued requests for the newest — a format is something the
/// user asked for once, not a stream sampled at keystroke rate.
pub struct FormatWorker {
    requests: Sender<Request>,
    results: Receiver<Formatted>,
}

impl Default for FormatWorker {
    fn default() -> Self {
        Self::spawn()
    }
}

impl FormatWorker {
    pub fn spawn() -> Self {
        Self::spawn_with(None)
    }

    /// Like [`spawn`](Self::spawn), but `waker` is called from the worker thread after every result
    /// has been sent, so a loop that sleeps until input still collects it.
    pub fn spawn_with(waker: Option<Waker>) -> Self {
        let (requests, incoming) = channel::<Request>();
        let (outgoing, results) = channel::<Formatted>();

        std::thread::spawn(move || {
            while let Ok(request) = incoming.recv() {
                let version = request.version;
                let (text, problem) = run(&request);
                if outgoing
                    .send(Formatted {
                        version,
                        text,
                        problem,
                    })
                    .is_err()
                {
                    break;
                }
                if let Some(wake) = &waker {
                    wake();
                }
            }
        });

        Self { requests, results }
    }

    pub fn request(&self, request: Request) {
        let _ = self.requests.send(request);
    }

    /// The newest result, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<Formatted> {
        let mut latest = None;
        while let Ok(formatted) = self.results.try_recv() {
            latest = Some(formatted);
        }
        latest
    }
}

/// Runs one formatter over one buffer.
fn run(request: &Request) -> (Option<String>, Option<String>) {
    use std::io::Write;

    let mut command = crate::process::command_at(&request.executable);
    command
        .args(request.arguments())
        // From the project root, so a formatter finds the `.prettierrc` or `pyproject.toml` that
        // decides how it behaves. Run from wherever the desktop launched the editor, prettier would
        // silently use its defaults and reformat a whole project against the wrong style.
        .current_dir(&request.root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return (
                None,
                Some(format!(
                    "{} would not start: {error}",
                    request.formatter.program
                )),
            );
        }
    };

    // Written on a thread of its own: a formatter that starts printing before it has read all of
    // stdin will fill the pipe buffer and stop, and a parent that is still writing then deadlocks
    // against it. Large files are exactly when this happens.
    if let Some(mut stdin) = child.stdin.take() {
        let text = request.text.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            return (
                None,
                Some(format!("{} failed: {error}", request.formatter.program)),
            );
        }
    };

    if !output.status.success() {
        // The formatter's own message, which for a syntax error names the line. Far more use than
        // "formatting failed", and the reason stderr is captured rather than discarded.
        let complaint = String::from_utf8_lossy(&output.stderr);
        let first = complaint
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no output");
        return (
            None,
            Some(format!("{}: {first}", request.formatter.program)),
        );
    }

    match String::from_utf8(output.stdout) {
        // A formatter that returns nothing has not formatted the file to empty; it has failed in a
        // way it did not report. Replacing the buffer with that would destroy the file.
        Ok(text) if text.is_empty() => (
            None,
            Some(format!(
                "{} produced no output, so nothing was changed",
                request.formatter.program
            )),
        ),
        Ok(text) => (Some(text), None),
        Err(_) => (
            None,
            Some(format!(
                "{} produced output that is not UTF-8",
                request.formatter.program
            )),
        ),
    }
}

impl Request {
    /// The arguments, with the file name appended where the program needs one.
    ///
    /// `prettier --stdin-filepath`, `clang-format --assume-filename` and `biome --stdin-file-path`
    /// all take the name as the *next* argument: they format from stdin but need the path to pick a
    /// parser, and given no name prettier refuses outright with "No parser could be inferred".
    fn arguments(&self) -> Vec<String> {
        let mut args = self.formatter.args.clone();
        if args.last().is_some_and(|last| {
            matches!(
                last.as_str(),
                "--stdin-filepath" | "--assume-filename" | "--stdin-file-path"
            )
        }) {
            args.push(self.path.to_string_lossy().to_string());
        }
        args
    }
}

/// The smallest replacement that turns `before` into `after`.
///
/// Returned as one edit over the span that actually differs, rather than replacing the whole
/// document. That matters for more than tidiness: a whole-buffer replacement moves every anchor,
/// which means the cursor, the selections and the scroll position all land somewhere arbitrary
/// after a format that changed two lines.
///
/// Common prefix and suffix only — not a real diff. A formatter that reflows a whole file produces
/// one big edit either way, and one that changes a line in the middle produces a small one, which
/// covers what actually happens. A formatter that changes the *first* and *last* lines and nothing
/// between still yields a whole-file edit; that is the honest limitation of this method.
pub fn narrow(before: &str, after: &str) -> Option<(std::ops::Range<usize>, String)> {
    if before == after {
        return None;
    }

    let prefix = before
        .char_indices()
        .zip(after.chars())
        .take_while(|((_, a), b)| a == b)
        .count();

    // Measured from the end, and never allowed to overlap the prefix — two spans that meet in the
    // middle would describe a negative range.
    let remaining = before.chars().count().min(after.chars().count()) - prefix;
    let suffix = before
        .chars()
        .rev()
        .zip(after.chars().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(remaining);

    let end = before.chars().count() - suffix;
    let replacement: String = after
        .chars()
        .skip(prefix)
        .take(after.chars().count() - suffix - prefix)
        .collect();

    Some((prefix..end, replacement))
}

/// Where a formatter is, honouring the same path rules a configured language server gets.
pub fn resolve(formatter: &Formatter, root: &Path, home: Option<&Path>) -> Option<PathBuf> {
    dacode_lsp::resolve_command(&formatter.program, root, home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_format_wakes_whoever_is_waiting() {
        // A formatted buffer that waits in the channel for the next keypress is a save that seems
        // to have done nothing. A program that cannot start is the cheapest way to get an answer
        // without needing any formatter installed.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let woken = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let worker = FormatWorker::spawn_with(Some(std::sync::Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        let root = std::env::temp_dir();
        worker.request(Request {
            formatter: Formatter {
                program: "dacode-no-such-formatter".into(),
                args: Vec::new(),
                language: "Nothing".into(),
                install: String::new(),
                configured: false,
            },
            executable: root.join("dacode-no-such-formatter"),
            path: root.join("a.txt"),
            root,
            text: "x\n".into(),
            version: Version(1),
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while woken.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            woken.load(Ordering::SeqCst) > 0,
            "the waker was never called"
        );
        let answer = worker.poll().expect("the answer is in the channel");
        assert!(answer.problem.is_some(), "a program that will not start");
    }

    #[test]
    fn a_file_finds_the_formatters_for_its_language() {
        let none = [];
        let js = formatters_for("src/app.tsx", &none);
        assert_eq!(js[0].program, "prettier");
        assert!(js.len() > 1, "and a fallback for a machine without it");

        assert_eq!(formatters_for("main.rs", &none)[0].program, "rustfmt");
        assert_eq!(formatters_for("a.py", &none)[0].program, "ruff");
        assert_eq!(formatters_for("README.md", &none)[0].program, "prettier");
        assert!(
            formatters_for("notes.txt", &none).is_empty(),
            "and a language dacode knows nothing about gets nothing rather than a guess"
        );
        assert!(
            formatters_for(".gitignore", &none).is_empty(),
            "a dotfile has no extension"
        );
    }

    #[test]
    fn a_configured_formatter_is_tried_before_the_built_in_one() {
        // The venv case again: a project pinning its own black must not also lose the fallback.
        let mine = configure("python", ".venv/bin/black", &["-".into()], &[]).expect("valid");
        assert_eq!(mine.extensions, vec!["py", "pyi"]);

        let all = formatters_for("a.py", std::slice::from_ref(&mine));
        assert_eq!(all[0].program, ".venv/bin/black");
        assert!(all[0].configured);
        assert!(all.len() > 1 && !all[1].configured, "{all:?}");

        assert!(
            formatters_for("a.rs", std::slice::from_ref(&mine))
                .iter()
                .all(|f| !f.configured),
            "and it does not leak into another language"
        );
    }

    #[test]
    fn a_language_dacode_does_not_know_must_say_which_files_it_formats() {
        let refused = configure("zig", "zig", &[], &[]).expect_err("no extensions");
        assert!(refused.contains("extensions"), "{refused}");

        let ok = configure(
            "zig",
            "zig",
            &["fmt".into(), "--stdin".into()],
            &[".ZIG".into()],
        )
        .expect("with extensions");
        assert_eq!(ok.extensions, vec!["zig"]);
        assert_eq!(
            formatters_for("build.zig", std::slice::from_ref(&ok)).len(),
            1
        );

        assert!(
            configure("python", "  ", &[], &[]).is_err(),
            "and a command is required"
        );
    }

    #[test]
    fn the_file_name_is_appended_only_where_the_program_asked_for_one() {
        // prettier refuses outright without it — "No parser could be inferred" — and gofmt would
        // read a stray argument as a file to format instead of stdin.
        let request = |program: &str, args: &[&str]| Request {
            formatter: Formatter {
                program: program.into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
                language: String::new(),
                install: String::new(),
                configured: false,
            },
            executable: PathBuf::from(program),
            path: PathBuf::from("/p/src/app.ts"),
            root: PathBuf::from("/p"),
            text: String::new(),
            version: Version(1),
        };

        assert_eq!(
            request("prettier", &["--stdin-filepath"]).arguments(),
            vec!["--stdin-filepath", "/p/src/app.ts"]
        );
        assert_eq!(request("gofmt", &[]).arguments(), Vec::<String>::new());
        assert_eq!(request("black", &["-", "-q"]).arguments(), vec!["-", "-q"]);
    }

    #[test]
    fn only_the_part_that_changed_is_replaced() {
        // A whole-buffer replacement moves every anchor, so the cursor, the selections and the
        // scroll position all land somewhere arbitrary after a format that touched two lines.
        let before = "fn main() {\n    let x=1;\n    let y = 2;\n}\n";
        let after = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";

        let (range, replacement) = narrow(before, after).expect("they differ");
        assert!(
            range.start > 10 && range.end < before.chars().count(),
            "the untouched lines on both sides are outside the edit: {range:?}"
        );

        // And applying it reproduces the formatter's output exactly, which is the only thing that
        // makes narrowing safe.
        let mut rebuilt: String = before.chars().take(range.start).collect();
        rebuilt.push_str(&replacement);
        rebuilt.extend(before.chars().skip(range.end));
        assert_eq!(rebuilt, after);
    }

    #[test]
    fn identical_text_is_not_an_edit_at_all() {
        // A format that changed nothing must not dirty the buffer, or `format_on_save` would mark
        // every already-formatted file as modified the moment it was saved.
        assert_eq!(narrow("same\n", "same\n"), None);
    }

    #[test]
    fn narrowing_survives_text_that_is_all_prefix_or_all_suffix() {
        // The case that produces a negative range if the two spans are allowed to overlap: one
        // string is a prefix of the other, so every character matches from both ends at once.
        for (before, after) in [
            ("abc", "abcdef"),
            ("abcdef", "abc"),
            ("", "new"),
            ("old", ""),
            ("aaa", "aaaa"),
        ] {
            let (range, replacement) = narrow(before, after).expect("they differ");
            assert!(
                range.start <= range.end,
                "{before:?} → {after:?}: {range:?}"
            );

            let mut rebuilt: String = before.chars().take(range.start).collect();
            rebuilt.push_str(&replacement);
            rebuilt.extend(before.chars().skip(range.end));
            assert_eq!(rebuilt, after, "{before:?} → {after:?}");
        }
    }

    #[test]
    fn a_formatter_that_fails_changes_nothing_and_says_why() {
        // The program's own message names the line for a syntax error, which is worth far more than
        // "formatting failed" — and is the reason stderr is captured rather than discarded.
        let (program, args) = if cfg!(windows) {
            (
                "cmd",
                vec!["/c".to_string(), "echo broken 1>&2 & exit 1".to_string()],
            )
        } else {
            (
                "sh",
                vec!["-c".to_string(), "echo broken >&2; exit 1".to_string()],
            )
        };
        let Some(executable) = dacode_lsp::resolve(program) else {
            eprintln!("skipping: no {program} on this machine");
            return;
        };

        let (text, problem) = run(&Request {
            formatter: Formatter {
                program: program.into(),
                args,
                language: String::new(),
                install: String::new(),
                configured: false,
            },
            executable,
            path: PathBuf::from("x.rs"),
            root: std::env::temp_dir(),
            text: "anything".into(),
            version: Version(1),
        });

        assert!(text.is_none(), "nothing is applied");
        let problem = problem.expect("and it says why");
        assert!(problem.contains("broken"), "{problem}");
    }

    #[test]
    fn a_formatter_that_returns_nothing_is_refused() {
        // Empty output is a failure the program did not report, not a file formatted to nothing.
        // Applying it would destroy the buffer.
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "exit 0".to_string()])
        } else {
            ("sh", vec!["-c".to_string(), "exit 0".to_string()])
        };
        let Some(executable) = dacode_lsp::resolve(program) else {
            eprintln!("skipping: no {program} on this machine");
            return;
        };

        let (text, problem) = run(&Request {
            formatter: Formatter {
                program: program.into(),
                args,
                language: String::new(),
                install: String::new(),
                configured: false,
            },
            executable,
            path: PathBuf::from("x.rs"),
            root: std::env::temp_dir(),
            text: "fn main() {}\n".into(),
            version: Version(1),
        });

        assert!(text.is_none());
        assert!(problem.unwrap().contains("no output"));
    }

    #[test]
    fn a_real_formatter_formats_a_real_buffer() {
        // The whole feature, against a program that actually is a formatter. `cat` proves the pipe
        // works; only rustfmt proves the arguments in the table are the ones that make it read
        // stdin and write stdout rather than rewriting a file or printing a diff.
        let formatter = formatters_for("main.rs", &[])
            .into_iter()
            .next()
            .expect("rust has a formatter in the table");
        let Some(executable) = dacode_lsp::resolve(&formatter.program) else {
            eprintln!("skipping: rustfmt is not installed");
            return;
        };

        let messy = "fn  main( ) {\nlet x=1;\n}\n";
        let (text, problem) = run(&Request {
            formatter,
            executable,
            path: PathBuf::from("main.rs"),
            root: std::env::temp_dir(),
            text: messy.into(),
            version: Version(1),
        });

        assert_eq!(problem, None);
        let text = text.expect("formatted");
        assert!(text.contains("fn main()"), "{text}");
        assert!(text.contains("let x = 1;"), "{text}");

        // And the edit that carries it back is narrowed to what changed, not the whole file.
        let (range, _) = narrow(messy, &text).expect("it differs");
        assert!(range.start > 0, "the leading `fn` is untouched: {range:?}");
    }

    #[test]
    fn text_goes_through_stdin_and_comes_back_from_stdout() {
        // The whole contract, against a program every platform has. A formatter is never given the
        // *path*: one that rewrote the file on disk would leave a dirty buffer disagreeing with it.
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "more".to_string()])
        } else {
            ("cat", Vec::new())
        };
        let Some(executable) = dacode_lsp::resolve(program) else {
            eprintln!("skipping: no {program} on this machine");
            return;
        };

        let (text, problem) = run(&Request {
            formatter: Formatter {
                program: program.into(),
                args,
                language: String::new(),
                install: String::new(),
                configured: false,
            },
            executable,
            path: PathBuf::from("x.rs"),
            root: std::env::temp_dir(),
            text: "fn main() {}\n".into(),
            version: Version(1),
        });

        assert_eq!(problem, None);
        assert_eq!(text.unwrap().trim_end(), "fn main() {}");
    }
}

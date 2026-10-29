//! Installing a language server from inside the editor.
//!
//! # Why this exists, and why it is not a bundle
//!
//! DaCode has no marketplace, and the obvious answer to "no Python language server" is to ship every
//! server in the installer. That is the wrong trade: rust-analyzer alone is tens of megabytes,
//! pyright needs Node and gopls needs a Go toolchain, and a bundle is stale the day after release
//! while installing nine languages' tooling on a machine that wanted one.
//!
//! So the editor runs the *same* command it already told you to type. The message that said
//! "Install one with: rustup component add rust-analyzer" is the thing it will run, derived from the
//! same argv, so what is shown and what happens cannot drift apart.
//!
//! # What it will not do
//!
//! It never runs a package manager that needs root, never invokes a shell, and never installs
//! anything without being told twice — the first request explains the command and arms it, the
//! second runs it. Where no single command is honest, [`dacode_lsp::ServerSpec::installers`] is empty
//! and the editor gives advice instead of a button that cannot work. An editor that quietly runs
//! package managers is a worse thing to have than one that says what to type.
//!
//! And "the installer exited zero" is not "it worked": on Windows `pip` and `go install` both write
//! to directories PATH does not cover, so success is checked by looking for the program afterwards.
//!
//! # The one rule
//!
//! `rustup component add` takes tens of seconds. It runs on a worker thread and the UI polls, like
//! the formatter and the syntax parser. Output is collected and reported at the end rather than
//! streamed: progress from a package manager is carriage-return animation that means nothing once
//! it is text, and the useful report is "it worked" or the last few lines of why it did not.

use crate::wake::Waker;
use dacode_lsp::Installer;
use std::sync::mpsc::{Receiver, Sender, channel};

/// What to install, and what to call it on screen.
#[derive(Debug, Clone)]
pub struct Request {
    pub installer: Installer,
    /// The server's own program, so the editor can check afterwards whether it really appeared.
    pub program: String,
    /// "Python", for the sentence.
    pub language: String,
}

/// How it went.
#[derive(Debug, Clone)]
pub struct Installed {
    pub program: String,
    pub language: String,
    pub ok: bool,
    /// One line for the message box. Never the whole log: a package manager's output is pages, and
    /// the last thing it said before failing is the part worth reading.
    pub message: String,
}

/// How much of a failing installer's output to keep, in lines from the end.
const TAIL_LINES: usize = 4;

pub struct InstallWorker {
    requests: Sender<Request>,
    results: Receiver<Installed>,
    /// How many requests are still owed an answer, so the event loop knows not to sleep. Without it
    /// the reply lands in the channel and nothing wakes to read it, and the editor looks frozen
    /// until the next keystroke.
    outstanding: std::cell::Cell<usize>,
}

impl Default for InstallWorker {
    fn default() -> Self {
        Self::spawn()
    }
}

impl InstallWorker {
    pub fn spawn() -> Self {
        Self::spawn_with(None)
    }

    /// Like [`spawn`](Self::spawn), but `waker` is called from the worker thread after every result
    /// has been sent, so a loop that sleeps until input still collects it.
    pub fn spawn_with(waker: Option<Waker>) -> Self {
        let (requests, incoming) = channel::<Request>();
        let (outgoing, results) = channel::<Installed>();

        std::thread::spawn(move || {
            while let Ok(request) = incoming.recv() {
                if outgoing.send(run(&request)).is_err() {
                    break;
                }
                if let Some(wake) = &waker {
                    wake();
                }
            }
        });

        Self {
            requests,
            results,
            outstanding: std::cell::Cell::new(0),
        }
    }

    pub fn request(&self, request: Request) {
        self.outstanding.set(self.outstanding.get() + 1);
        let _ = self.requests.send(request);
    }

    /// Whether an answer is still owed.
    pub fn busy(&self) -> bool {
        self.outstanding.get() > 0
    }

    /// The next result, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<Installed> {
        match self.results.try_recv() {
            Ok(installed) => {
                self.outstanding
                    .set(self.outstanding.get().saturating_sub(1));
                Some(installed)
            }
            Err(_) => None,
        }
    }
}

/// Runs one installer to completion.
///
/// `output()` rather than `spawn()` plus reads: it waits for the child and collects both pipes
/// without the deadlock that comes of reading one pipe while the other fills. This is a worker
/// thread, so waiting is exactly what it is for.
fn run(request: &Request) -> Installed {
    let installer = &request.installer;

    // Checked first, so "npm is not installed" is the answer rather than an operating-system error
    // number that names no cause. This is also why `needs` is a field: the program that installs a
    // server is usually not the server, and the thing missing is usually the *toolchain*.
    // DaCode as its own installer is the running executable, whatever PATH says.
    let resolved = if installer.needs == dacode_lsp::SELF {
        std::env::current_exe().ok()
    } else {
        dacode_lsp::resolve(&installer.needs)
    };
    let Some(resolved) = resolved else {
        return Installed {
            program: request.program.clone(),
            language: request.language.clone(),
            ok: false,
            message: format!(
                "{} is not installed, and `{}` needs it. Install {} first, or name a server you \
                 already have with dacode.server(…)",
                installer.needs,
                installer.line(),
                installer.needs
            ),
        };
    };

    let mut command = crate::process::command_at(&resolved);
    command.args(&installer.args);

    match command.output() {
        // A zero exit is not the same as "DaCode can now run it", and the difference is common enough
        // to be the normal case on two platforms. `pip install --user` on Windows writes scripts to
        // a directory Python does not put on PATH and says so in a warning nobody reads; `go
        // install` writes to `~/go/bin`, which the Windows MSI has never added. Reporting success
        // there would leave the editor claiming a server it cannot start — so the check is whether
        // the program is *findable*, which is the same question that made it missing to begin with.
        Ok(output) if output.status.success() => match dacode_lsp::resolve(&request.program) {
            Some(_) => Installed {
                program: request.program.clone(),
                language: request.language.clone(),
                ok: true,
                message: format!(
                    "installed {} — {} has a language server now",
                    request.program, request.language
                ),
            },
            None => Installed {
                program: request.program.clone(),
                language: request.language.clone(),
                ok: false,
                message: format!(
                    "`{}` succeeded, but {} is still not on PATH — the installer put it somewhere \
                     PATH does not cover. Add that directory to PATH, or name the program with \
                     dacode.server(\"…\", {{ command = \"…\" }})",
                    installer.line(),
                    request.program
                ),
            },
        },
        Ok(output) => Installed {
            program: request.program.clone(),
            language: request.language.clone(),
            ok: false,
            message: format!(
                "`{}` failed: {}",
                installer.line(),
                tail(&output.stderr, &output.stdout)
            ),
        },
        Err(error) => Installed {
            program: request.program.clone(),
            language: request.language.clone(),
            ok: false,
            message: format!("could not run `{}`: {error}", installer.line()),
        },
    }
}

/// The last few meaningful lines of an installer's output.
///
/// stderr first, because that is where a package manager puts the reason; stdout only if stderr said
/// nothing, which npm sometimes does. Carriage returns are cut to their last segment: a progress bar
/// redrawn in place is one line holding forty frames of itself, and printing all of them fills the
/// message box with noise.
fn tail(stderr: &[u8], stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(if stderr.iter().any(|b| !b.is_ascii_whitespace()) {
        stderr
    } else {
        stdout
    });

    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.rsplit('\r').next().unwrap_or(line).trim())
        .filter(|line| !line.is_empty())
        .collect();

    if lines.is_empty() {
        return "it said nothing about why".to_string();
    }
    lines[lines.len().saturating_sub(TAIL_LINES)..].join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installer(needs: &str) -> Installer {
        Installer {
            program: needs.to_string(),
            args: vec!["install".into(), "something".into()],
            needs: needs.to_string(),
        }
    }

    #[test]
    fn a_missing_toolchain_is_named_rather_than_reported_as_an_os_error() {
        // The failure this exists for: `npm install -g pyright` on a machine with no Node fails with
        // "program not found", which names neither npm nor the fact that Node is the thing to go and
        // get. Someone reading that learns nothing they can act on.
        let request = Request {
            installer: installer("definitely-not-a-real-program-xyz"),
            program: "pyright-langserver".into(),
            language: "Python".into(),
        };
        let result = run(&request);

        assert!(!result.ok);
        assert!(
            result
                .message
                .contains("definitely-not-a-real-program-xyz is not installed"),
            "got: {}",
            result.message
        );
    }

    #[test]
    fn a_finished_install_wakes_whoever_is_waiting() {
        // An install takes tens of seconds, which is exactly when the editor is idle. Its verdict
        // must not wait for a keypress.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let woken = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let worker = InstallWorker::spawn_with(Some(std::sync::Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        worker.request(Request {
            installer: installer("definitely-not-a-real-program-xyz"),
            program: "pyright-langserver".into(),
            language: "Python".into(),
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while woken.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            woken.load(Ordering::SeqCst) > 0,
            "the waker was never called"
        );
        assert!(worker.poll().is_some_and(|result| !result.ok));
    }

    #[test]
    fn a_failing_installer_reports_what_it_last_said() {
        // A package manager that fails prints pages; the last few lines are the reason. Driven
        // through a program every platform guarantees, per this project's rule about naming one the
        // machine happens to have.
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "exit 1".to_string()])
        } else {
            ("sh", vec!["-c".to_string(), "exit 1".to_string()])
        };
        let request = Request {
            installer: Installer {
                program: program.into(),
                args,
                needs: program.into(),
            },
            program: "gopls".into(),
            language: "Go".into(),
        };
        let result = run(&request);

        assert!(!result.ok, "a non-zero exit is a failure");
        assert!(result.message.contains("failed"), "got: {}", result.message);
    }

    #[test]
    fn a_progress_bar_does_not_become_forty_lines_of_message() {
        let noisy = b"downloading\r 10%\r 50%\r 100%\nfailed: no such package\n";
        let kept = tail(noisy, b"");
        assert!(kept.contains("no such package"));
        assert!(
            !kept.contains("10%"),
            "the redrawn progress line came through: {kept}"
        );
    }

    #[test]
    fn stdout_is_used_when_stderr_said_nothing() {
        assert_eq!(tail(b"   \n", b"the actual reason\n"), "the actual reason");
    }
}

#[cfg(test)]
mod table_tests {
    /// Every server the editor offers to install must name a real toolchain and must not ask for
    /// root.
    ///
    /// The list is the part most likely to rot: a package manager renames a flag, an ecosystem moves
    /// from `pip` to `uv`, and nothing in a compiler notices. This does not check the commands are
    /// *current* — only a person with the docs open can do that — but it does pin the rules that
    /// made them safe to run at all.
    #[test]
    fn no_offered_install_needs_root_or_a_shell() {
        // Not exhaustive, and deliberately so: it names the ways this has actually gone wrong
        // elsewhere rather than pretending to a general theory of dangerous commands.
        const FORBIDDEN: [&str; 6] = ["sudo", "su", "doas", "runas", "pkexec", "sh"];

        for language in dacode_lsp::LANGUAGES {
            for spec in language.specs() {
                for installer in &spec.installers {
                    assert!(
                        !FORBIDDEN.contains(&installer.program.as_str()),
                        "{} would install {} as another user",
                        installer.line(),
                        spec.program
                    );
                    assert!(
                        !installer.needs.is_empty(),
                        "{} names no toolchain to check for first, so a missing one would surface \
                         as an unreadable spawn error",
                        installer.line()
                    );
                    // A shell metacharacter in an argv element is a sign someone wrote a shell line
                    // and split it by hand — which is the thing the argv form exists to prevent.
                    for arg in &installer.args {
                        assert!(
                            !arg.contains('|') && !arg.contains('&') && !arg.contains('>'),
                            "`{arg}` looks like shell, and nothing here runs a shell"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_language_with_no_safe_command_gives_advice_instead_of_an_empty_offer() {
        // clangd, lua-language-server and the Dart SDK. An editor that offered a button for these
        // would be offering one that cannot work: they come from a system package manager whose
        // name differs per distribution and which wants root.
        for language in dacode_lsp::LANGUAGES {
            for spec in language.specs() {
                if spec.installers.is_empty() {
                    assert!(
                        !spec.install.trim().is_empty(),
                        "{} for {} can neither be installed nor explained",
                        spec.program,
                        language.name
                    );
                }
            }
        }
    }
}

//! The native folder picker.
//!
//! Shells out to whatever dialog the desktop already provides rather than linking a toolkit. vex
//! draws its own interface with `wgpu` and has no GTK or Qt in it; pulling one in — or `rfd`, which
//! pulls one in for you — to show one dialog would add a second UI stack, a second theme engine and
//! a second event loop to an editor whose premise is staying light.
//!
//! **It runs on a thread.** A dialog is a subprocess the user might leave open for a minute, and
//! waiting for it on the UI thread would freeze the editor behind it — precisely the unbounded work
//! §1.1 forbids. The result arrives on a channel and is picked up by the same poll that services the
//! syntax worker and the language server.
//!
//! When no dialog exists the caller falls back to typing a path, which is why [`is_available`] is
//! public: an editor that silently does nothing when you press a key is worse than one that asks you
//! to type.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

/// Whether any folder dialog is available.
pub fn is_available() -> bool {
    which("zenity") || which("kdialog") || portal_available()
}

/// Which dialog would be used, for reporting.
///
/// `zenity` and `kdialog` come before the portal deliberately. Both are one synchronous command that
/// prints a path on stdout — a contract that is easy to be sure of. The portal is the more modern
/// interface and the one that works under Flatpak, but driving it without a D-Bus library means
/// spawning a monitor, racing a signal against a method call, and parsing the result out of a text
/// stream. It is kept as the fallback for systems with neither dialog installed, where it is better
/// than refusing, but it is not what runs when something simpler is present.
pub fn describe() -> &'static str {
    if which("zenity") {
        "zenity"
    } else if which("kdialog") {
        "kdialog"
    } else if portal_available() {
        "desktop portal"
    } else {
        "none"
    }
}

/// A folder choice in progress.
pub struct FolderPicker {
    result: Receiver<Option<PathBuf>>,
}

impl FolderPicker {
    /// Opens the dialog, starting at `start`. Returns immediately.
    pub fn open(start: PathBuf) -> Self {
        let (sender, result) = channel();

        std::thread::spawn(move || {
            let chosen = choose(&start);
            // A closed channel means the editor moved on; nothing to do about that here.
            let _ = sender.send(chosen);
        });

        Self { result }
    }

    /// The chosen folder, once the dialog closes. `Some(None)` means the user cancelled.
    pub fn poll(&self) -> Option<Option<PathBuf>> {
        self.result.try_recv().ok()
    }
}

fn which(program: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {program}"))
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Whether the desktop portal is on the session bus.
///
/// Checked by asking the bus rather than by looking for a binary: the portal is a service that may be
/// activatable without its executable being on `PATH`.
fn portal_available() -> bool {
    which("gdbus")
        && std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.DBus",
                "--object-path",
                "/org/freedesktop/DBus",
                "--method",
                "org.freedesktop.DBus.NameHasOwner",
                "org.freedesktop.portal.Desktop",
            ])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains("true"))
            .unwrap_or(false)
}

/// Runs the first available dialog and returns what it chose.
///
/// One dialog, whatever happens. A chain that opened a second one after the first was cancelled would
/// be worse than not having the feature.
fn choose(start: &std::path::Path) -> Option<PathBuf> {
    if which("zenity") {
        return via_command(
            "zenity",
            &[
                "--file-selection",
                "--directory",
                "--title=Open folder in vex",
                &format!("--filename={}/", start.display()),
            ],
        );
    }

    if which("kdialog") {
        return via_command(
            "kdialog",
            &["--getexistingdirectory", &start.display().to_string()],
        );
    }

    if portal_available() {
        return via_portal(start);
    }

    None
}

/// A dialog that prints the chosen path on stdout, which both zenity and kdialog do.
fn via_command(program: &str, args: &[&str]) -> Option<PathBuf> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        // A non-zero exit is a cancel, which is not an error worth reporting.
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = path.strip_prefix("file://").unwrap_or(&path);
    (!path.is_empty()).then(|| PathBuf::from(decode(path)))
}

/// The portal's file chooser, over `gdbus`.
///
/// The portal replies asynchronously: the method call returns a request handle and the answer arrives
/// as a `Response` signal. `gdbus monitor` is started *before* the call so the signal cannot be missed
/// in between — which is the whole difficulty of driving this interface from a shell.
fn via_portal(start: &std::path::Path) -> Option<PathBuf> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let mut monitor = Command::new("gdbus")
        .args([
            "monitor",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let call = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.FileChooser.OpenFile",
            "",
            "Open folder in vex",
            &format!(
                "{{'directory': <true>, 'modal': <true>, 'current_folder': <b'{}\\0'>}}",
                start.display()
            ),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok();

    if call.as_ref().is_none_or(|out| !out.status.success()) {
        let _ = monitor.kill();
        return None;
    }

    // Read signals until the response arrives — or until the dialog has plainly been abandoned. An
    // unbounded read would hold this thread and a `gdbus monitor` process for the life of the editor
    // if the signal never came, which is a leak rather than a hang, but a leak with a subprocess in
    // it.
    let stdout = monitor.stdout.take()?;
    let reader = BufReader::new(stdout);
    let mut chosen = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);

    for line in reader.lines().map_while(Result::ok) {
        if std::time::Instant::now() > deadline {
            break;
        }
        if !line.contains("org.freedesktop.portal.Request.Response") {
            continue;
        }
        // `uint32 0` is success; anything else is a cancel.
        if let Some(uri) = line.split("'file://").nth(1) {
            if let Some(end) = uri.find('\'') {
                chosen = Some(PathBuf::from(decode(&uri[..end])));
            }
        }
        break;
    }

    let _ = monitor.kill();
    let _ = monitor.wait();
    chosen
}

/// Percent-decodes a URI path. The portal returns `file://` URIs, and a folder with a space in it is
/// not unusual.
fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_paths_are_decoded() {
        assert_eq!(decode("/home/me/my%20project"), "/home/me/my project");
        assert_eq!(decode("/plain/path"), "/plain/path");
        // A stray percent is left alone rather than eating the characters after it.
        assert_eq!(decode("/100%"), "/100%");
        assert_eq!(decode("/a%zz"), "/a%zz");
    }

    #[test]
    fn which_finds_a_program_that_exists_and_not_one_that_does_not() {
        assert!(which("sh"), "sh is on every system this runs on");
        assert!(!which("definitely-not-a-real-program-xyzzy"));
    }

    #[test]
    fn describe_names_something_or_says_none() {
        // Both answers are valid depending on the machine; what matters is that it is one of them,
        // because the caller shows this string to the user.
        let named = describe();
        assert!(
            ["desktop portal", "zenity", "kdialog", "none"].contains(&named),
            "unexpected description: {named}"
        );
        assert_eq!(named != "none", is_available());
    }
}

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
    // Windows always has one: PowerShell ships with the OS, and the shell's folder browser needs no
    // extra install.
    cfg!(windows) || which("zenity") || which("kdialog") || portal_available()
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
    if cfg!(windows) {
        "windows shell"
    } else if which("zenity") {
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
    // `PATH` is walked directly rather than shelling out to `command -v`, which does not exist on
    // Windows, and to `where`, which is slower and prints to stdout for a question with a yes/no
    // answer.
    crate::paths::program_on_path(program)
}

/// Whether the desktop portal is on the session bus.
///
/// Checked by asking the bus rather than by looking for a binary: the portal is a service that may be
/// activatable without its executable being on `PATH`.
fn portal_available() -> bool {
    which("gdbus")
        && crate::process::command("gdbus")
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
    // Compiled on every platform and chosen by a runtime check, not `#[cfg]`. A `#[cfg(windows)]`
    // body is never type-checked on Linux, and this is exactly the code that cannot be tested here —
    // the combination that ships a Windows build broken in a way nobody saw.
    if cfg!(windows) {
        return via_powershell(start);
    }

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

/// The Windows folder browser, through PowerShell.
///
/// `FolderBrowserDialog` first, `Shell.Application` second, and the order matters both ways. The Forms
/// dialog is the modern one — it has an address bar, it takes a typed path, and `SelectedPath` opens it
/// *at* a folder without confining it to one. It needs the .NET desktop assemblies, which are on every
/// desktop Windows but not on every edition, so the shell's COM browser stays as the fallback.
///
/// The fallback's fourth argument is the **root** of the tree it shows, not where it starts. Passing
/// the current project's parent there is what left the chooser unable to reach anything outside that
/// one folder — everything above it simply was not in the tree. It is `0` (the desktop, so the whole
/// machine) plus `BIF_EDITBOX`, which at least gives a box to type a path into.
///
/// The dialog is given an invisible topmost owner form. Without one it belongs to a PowerShell process
/// that has no window at all, and Windows is free to put it *behind* the editor — which reads as a
/// freeze, since the editor is sitting there saying "choose a folder…".
///
/// The path is printed on stdout, the same contract zenity and kdialog use, so the caller does not
/// care which of the two ran.
fn via_powershell(start: &std::path::Path) -> Option<PathBuf> {
    via_command(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            // The Forms dialog is a COM host and refuses to run on a multi-threaded apartment. Windows
            // PowerShell already defaults to STA; saying so means the dialog does not depend on that
            // staying true.
            "-Sta",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &powershell_script(start),
        ],
    )
}

/// The script itself, separated so it can be inspected from a machine that cannot run it — which is
/// every machine this is developed on. The Windows dialog is the one part of the editor nothing else
/// here can exercise.
fn powershell_script(start: &std::path::Path) -> String {
    format!(
        "$path = ''; \
         $start = '{}'; \
         try {{ \
           Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop; \
           $dialog = New-Object System.Windows.Forms.FolderBrowserDialog; \
           $dialog.Description = 'Open folder in vex'; \
           $dialog.ShowNewFolderButton = $true; \
           if (Test-Path -LiteralPath $start) {{ $dialog.SelectedPath = $start }}; \
           $owner = New-Object System.Windows.Forms.Form; \
           $owner.TopMost = $true; \
           $owner.ShowInTaskbar = $false; \
           $null = $owner.Handle; \
           $answer = $dialog.ShowDialog($owner); \
           $owner.Dispose(); \
           if ($answer -eq [System.Windows.Forms.DialogResult]::OK) {{ \
             $path = $dialog.SelectedPath \
           }} \
         }} catch {{ \
           $shell = New-Object -ComObject Shell.Application; \
           $folder = $shell.BrowseForFolder(0, 'Open folder in vex', 0x11, 0); \
           if ($folder -and $folder.Self) {{ $path = $folder.Self.Path }} \
         }}; \
         if ($path) {{ Write-Output $path }}",
        quote(start)
    )
}

/// A path as a PowerShell single-quoted string. Doubling the quote is the whole escaping rule for
/// them, and a folder with an apostrophe in it would otherwise end the string and run what followed.
fn quote(path: &std::path::Path) -> String {
    path.display().to_string().replace('\'', "''")
}

/// A dialog that prints the chosen path on stdout, which both zenity and kdialog do.
fn via_command(program: &str, args: &[&str]) -> Option<PathBuf> {
    // Through `process::command`, so the console-mode `powershell.exe` does not get a console window
    // of its own allocated in front of the editor — which is what made "open folder" look like it was
    // launching a terminal.
    let output = crate::process::command(program).args(args).output().ok()?;
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
    use std::process::Stdio;

    let mut monitor = crate::process::command("gdbus")
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

    let call = crate::process::command("gdbus")
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
    fn the_windows_dialog_opens_at_a_folder_rather_than_being_confined_to_it() {
        // The bug this guards: the starting directory was passed as `BrowseForFolder`'s *root*, which
        // is the top of the tree it shows — so the chooser could not reach anything above the folder
        // vex happened to be started in, and on a Start Menu launch that was the install directory.
        // The start belongs in `SelectedPath`; the root stays 0, the desktop.
        let script = powershell_script(std::path::Path::new("C:/Users/someone/code"));
        assert!(script.contains("$dialog.SelectedPath = $start"));
        assert!(
            script.contains("BrowseForFolder(0, 'Open folder in vex', 0x11, 0)"),
            "the fallback's root must stay 0: {script}"
        );
        assert!(
            !script.contains("BrowseForFolder(0, 'Open folder in vex', 0, '"),
            "the old rooted form is back: {script}"
        );
    }

    #[test]
    fn a_path_with_an_apostrophe_cannot_end_the_powershell_string() {
        // `C:\Users\O'Brien` is a real name, and an unescaped quote there would close the string and
        // run whatever came after it as PowerShell.
        let script = powershell_script(std::path::Path::new(r"C:\Users\O'Brien\code"));
        assert!(
            script.contains(r"$start = 'C:\Users\O''Brien\code'"),
            "{script}"
        );
    }

    #[test]
    fn which_finds_a_program_that_exists_and_not_one_that_does_not() {
        // The program has to be one the *platform* guarantees, not one this machine happens to
        // have. This said `sh` and passed everywhere it had ever run — including Windows CI, where
        // the runner image ships Git's `sh.exe` — and then failed on the first real Windows
        // developer machine, which does not. A test that fails for want of something unrelated is
        // testing the machine, exactly as `paths.rs` says.
        let known = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(which(known), "{known} is on every system this runs on");
        assert!(!which("definitely-not-a-real-program-xyzzy"));
    }

    #[test]
    fn describe_names_something_or_says_none() {
        // Both answers are valid depending on the machine; what matters is that it is one of them,
        // because the caller shows this string to the user.
        let named = describe();
        assert!(
            [
                "windows shell",
                "desktop portal",
                "zenity",
                "kdialog",
                "none"
            ]
            .contains(&named),
            "unexpected description: {named}"
        );
        assert_eq!(named != "none", is_available());
    }
}

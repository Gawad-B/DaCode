//! What is different about running on a Mac, as functions that take the platform as a parameter so
//! their rules have tests on the Linux machine nobody has a Mac beside.
//!
//! What is here, all of it about the person rather than the window:
//!
//! - **⌘ is Ctrl.** Every shortcut in the command table is written `ctrl-…`, and a Mac user reaches
//!   for ⌘S, ⌘P and ⌘⇧P without thinking. So ⌘ held counts as ctrl held, everywhere a binding is
//!   looked up — and physical Ctrl still counts too, because Vim's `ctrl-r`, `ctrl-d` and `ctrl-v`
//!   are Ctrl on every keyboard, and a shell is owed its `ctrl-c`. The menus therefore print `ctrl-s`,
//!   which is true: it works. They do not print `⌘S`, which would be nicer.
//! - **An app opened from the Finder gets launchd's `PATH`**, which is `/usr/bin:/bin:/usr/sbin:/sbin`
//!   and nothing else. Homebrew, `~/.cargo/bin` and `/usr/local/bin` are all missing from it, so
//!   `rust-analyzer`, `git` from Homebrew, every formatter and every npm language server were found
//!   when DaCode was started from a terminal and not when it was started from the Dock. The same
//!   `PATH` the user's own shell builds is asked for once, in `main`, before any thread exists
//!   ([`login_shell_path`]): their shell, as a login and interactive one so `.zprofile` and `.zshrc`
//!   both run, which is where nvm, pyenv and Homebrew's `shellenv` put themselves. That is the
//!   user's profile running on the thread about to open the window, so it is bounded:
//!   [`LOGIN_SHELL_TIMEOUT`], then it is killed and the directories a login shell would usually
//!   add ([`finder_path`]) are what DaCode has. A started-from-a-terminal editor already has the
//!   real `PATH` and skips all of it.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Whether a binding should see ctrl held: Ctrl itself, or ⌘ on a Mac.
pub fn ctrl_held(control: bool, command: bool, mac: bool) -> bool {
    control || (mac && command)
}

/// Which Option key works as Alt — the `option_as_alt` setting. On a Mac, Option is also how a
/// great many characters are typed: `@`, `{`, `[` and `|` on a German keyboard, `€` on most. An
/// Option that is Alt types none of them, and an Option that is not reaches none of the `alt-…`
/// bindings — `ƒ` arrives instead of `alt-f`. One key each, by default the left as Alt, is how
/// Terminal, iTerm and Alacritty settle the same question; the setting is for a layout that
/// needs the left one for typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionKey {
    #[default]
    Left,
    Right,
    Both,
    Neither,
}

impl OptionKey {
    pub const ALL: [OptionKey; 4] = [
        OptionKey::Left,
        OptionKey::Right,
        OptionKey::Both,
        OptionKey::Neither,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            OptionKey::Left => "left",
            OptionKey::Right => "right",
            OptionKey::Both => "both",
            OptionKey::Neither => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<OptionKey> {
        OptionKey::ALL.into_iter().find(|key| key.name() == name)
    }
}

/// The same choice in winit's terms, for the window.
#[cfg(target_os = "macos")]
pub fn winit_option(key: OptionKey) -> winit::platform::macos::OptionAsAlt {
    use winit::platform::macos::OptionAsAlt;
    match key {
        OptionKey::Left => OptionAsAlt::OnlyLeft,
        OptionKey::Right => OptionAsAlt::OnlyRight,
        OptionKey::Both => OptionAsAlt::Both,
        OptionKey::Neither => OptionAsAlt::None,
    }
}

/// Whether a binding should see alt held. Off a Mac, Alt is Alt. On one, with ⌘ or Ctrl also held
/// either Option is ⌥ — every Mac application reads ⌥⌘F the same whichever side was pressed, and
/// no character is typed with ⌘ down. Without them only the side the setting names is Alt, so the
/// other still types `@`. `left` and `right` are which Option keys are down; when the platform
/// could not say (`any` without either), the setting is taken at its word unless it is `Neither`.
pub fn alt_held(
    any: bool,
    left: bool,
    right: bool,
    chord: bool,
    setting: OptionKey,
    mac: bool,
) -> bool {
    if !mac || chord {
        return any;
    }
    if any && !left && !right {
        return setting != OptionKey::Neither;
    }
    match setting {
        OptionKey::Left => left,
        OptionKey::Right => right,
        OptionKey::Both => left || right,
        OptionKey::Neither => false,
    }
}

/// A key the Mac keeps for the window rather than the editor. With winit's default menu gone (it
/// quit without asking about unsaved files), nothing else provides these, and a Mac user presses
/// them without thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKey {
    /// ⌘H.
    Hide,
    /// ⌥⌘H.
    HideOthers,
    /// ⌘M.
    Minimise,
}

/// Which [`WindowKey`] a press is, if any. Only with ⌘ and *not* Ctrl: physical Ctrl+H is still the
/// editor's replace, which is how someone who wants it on that key keeps it. `character` is the
/// key without Option applied, as winit reports it for a left Option acting as Alt.
pub fn window_key(
    character: char,
    command: bool,
    control: bool,
    option: bool,
    shift: bool,
) -> Option<WindowKey> {
    if !command || control || shift {
        return None;
    }
    match (character.to_ascii_lowercase(), option) {
        ('h', false) => Some(WindowKey::Hide),
        ('h', true) => Some(WindowKey::HideOthers),
        ('m', false) => Some(WindowKey::Minimise),
        _ => None,
    }
}

/// Files the Finder asked DaCode to open — "Open With", a double-click on a file DaCode is the
/// default for, or one dropped on the Dock icon — waiting for the event loop to take them.
///
/// A queue rather than a call into the editor because macOS delivers them from inside AppKit,
/// sometimes before the window exists: the first one arrives while the application is still
/// finishing its launch. Platform-independent so the queue has tests; only a Mac ever fills it.
static OPENED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
static OPENED_WAKER: std::sync::OnceLock<crate::wake::Waker> = std::sync::OnceLock::new();

/// Queues paths to open and wakes the loop, which may be asleep with nothing else to do.
pub fn files_opened(paths: impl IntoIterator<Item = PathBuf>) {
    if let Ok(mut opened) = OPENED.lock() {
        opened.extend(paths);
    }
    if let Some(wake) = OPENED_WAKER.get() {
        wake();
    }
}

/// Everything queued since the last call, oldest first.
pub fn take_opened_files() -> Vec<PathBuf> {
    OPENED
        .lock()
        .map(|mut opened| std::mem::take(&mut *opened))
        .unwrap_or_default()
}

/// The waker [`files_opened`] calls. Set once, by `main`, after the event loop exists.
pub fn wake_for_opened_files(waker: crate::wake::Waker) {
    let _ = OPENED_WAKER.set(waker);
}

/// Teaches winit's application delegate `application:openURLs:`, which AppKit calls with the files
/// the Finder hands over. winit 0.30 does not implement it, and an application delegate that does
/// not respond to it is simply never told — the file is dropped and DaCode opens empty.
///
/// The method is added to winit's class at run time rather than by replacing the delegate, which
/// winit relies on for every window event. It must run after the event loop is built (that is when
/// the class exists) and before it runs (AppKit asks during launch, and then remembers the answer).
#[cfg(target_os = "macos")]
pub fn listen_for_opened_files() {
    use objc2::runtime::{AnyClass, AnyObject, Sel};
    use objc2::{msg_send, sel};

    extern "C" fn open_urls(_delegate: &AnyObject, _cmd: Sel, _app: &AnyObject, urls: &AnyObject) {
        let mut paths = Vec::new();
        // SAFETY: AppKit passes an `NSArray<NSURL *>`; `count`, `objectAtIndex:`, `isFileURL`,
        // `path` and `UTF8String` are the documented methods of those classes, and the C string
        // lives as long as the autoreleased `NSString` it came from, which outlives this loop body.
        unsafe {
            let count: usize = msg_send![urls, count];
            for index in 0..count {
                let url: *mut AnyObject = msg_send![urls, objectAtIndex: index];
                let Some(url) = url.as_ref() else { continue };
                let is_file: bool = msg_send![url, isFileURL];
                if !is_file {
                    continue;
                }
                let path: *mut AnyObject = msg_send![url, path];
                let Some(path) = path.as_ref() else { continue };
                let text: *const std::ffi::c_char = msg_send![path, UTF8String];
                if text.is_null() {
                    continue;
                }
                let text = std::ffi::CStr::from_ptr(text);
                paths.push(PathBuf::from(
                    <OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(text.to_bytes()),
                ));
            }
        }
        files_opened(paths);
    }

    let Some(class) = AnyClass::get("WinitApplicationDelegate") else {
        eprintln!("dacode: winit's application delegate was not found; Finder cannot open files");
        return;
    };
    let open: extern "C" fn(&AnyObject, Sel, &AnyObject, &AnyObject) = open_urls;
    // SAFETY: the signature matches `- (void)application:(NSApplication *)app
    // openURLs:(NSArray<NSURL *> *)urls`, whose type encoding is `v@:@@`, and `class_addMethod`
    // leaves the class untouched if winit ever implements the method itself.
    unsafe {
        let imp: objc2::ffi::IMP = Some(std::mem::transmute::<
            extern "C" fn(&AnyObject, Sel, &AnyObject, &AnyObject),
            unsafe extern "C" fn(),
        >(open));
        objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut objc2::ffi::objc_class,
            sel!(application:openURLs:).as_ptr(),
            imp,
            c"v@:@@".as_ptr(),
        );
    }
}

/// How long the user's shell may take to say what its `PATH` is. A profile that loads nvm takes
/// half a second; one that takes longer than this is stuck on something, and the window has been
/// waiting for it.
pub const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(3);

/// Printed either side of the `PATH`, so whatever an interactive profile prints — a fortune, a
/// greeting, a warning about a missing plugin — is not taken for part of it.
const PATH_MARK: &str = "__DACODE_PATH__";

/// Whether to ask the login shell: on a Mac, and only for a launch that did not come from a
/// terminal. A terminal sets `TERM`; launchd does not, and a terminal's `PATH` is already the one
/// its shell built.
pub fn wants_login_shell(term: Option<&OsStr>, mac: bool) -> bool {
    mac && term.is_none_or(OsStr::is_empty)
}

/// What the shell is asked to run. `printenv` rather than `echo $PATH` because fish keeps `PATH`
/// as a list and prints it with spaces; every shell's `printenv` prints the environment's colons.
pub fn login_shell_script() -> String {
    format!("echo {PATH_MARK}; /usr/bin/printenv PATH; echo {PATH_MARK}")
}

/// The `PATH` between the marks in what the shell printed, or `None` if it never got that far.
pub fn parse_login_path(output: &str) -> Option<OsString> {
    let mut parts = output.split(PATH_MARK);
    parts.next()?;
    let path = parts.next()?.trim();
    parts.next()?;
    (!path.is_empty()).then(|| OsString::from(path))
}

/// The `PATH` the user's login shell builds, or `None` if it could not be had within `timeout`.
/// `shell` is `$SHELL`, or the Mac's default when launchd left that unset too.
pub fn login_shell_path(shell: &Path, timeout: Duration) -> Option<OsString> {
    let mut child = crate::process::command_at(shell)
        .args(["-l", "-i", "-c", &login_shell_script()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // Polled rather than waited on with a thread: this runs before any thread exists, so that
    // `main` can still `set_var` the answer safely.
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    parse_login_path(&output)
}

/// What a login shell would have put on `PATH` that a Finder launch did not: `/etc/paths` and
/// `/etc/paths.d` (what macOS's own `path_helper` reads), then where Homebrew, rustup and pip put
/// programs. Appended after what is there, in that order, each once; `None` when nothing is missing.
///
/// `etc_paths` is the text of `/etc/paths` followed by each file in `/etc/paths.d`, one directory
/// per line, read by the caller so this has no filesystem to mock.
pub fn finder_path(path: Option<&OsStr>, home: &Path, etc_paths: &[String]) -> Option<OsString> {
    let mut parts: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let listed = etc_paths
        .iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from);
    let known = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
    ];
    // Only with a home directory: an empty one would add `/.cargo/bin`, a directory at the root.
    let personal = (!home.as_os_str().is_empty())
        .then(|| [home.join(".cargo/bin"), home.join(".local/bin")])
        .into_iter()
        .flatten();

    let before = parts.len();
    for dir in listed.chain(known).chain(personal) {
        if !parts.contains(&dir) {
            parts.push(dir);
        }
    }
    if parts.len() == before {
        return None;
    }
    std::env::join_paths(parts).ok()
}

/// `/etc/paths` and every file in `/etc/paths.d`, as [`finder_path`] wants them. Missing files
/// are simply absent.
pub fn read_etc_paths() -> Vec<String> {
    let mut texts: Vec<String> = std::fs::read_to_string("/etc/paths").into_iter().collect();
    let mut files: Vec<PathBuf> = std::fs::read_dir("/etc/paths.d")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    // `path_helper` reads them in name order; so does this, so the two agree on which wins.
    files.sort();
    texts.extend(
        files
            .iter()
            .filter_map(|file| std::fs::read_to_string(file).ok()),
    );
    texts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_counts_as_ctrl_on_a_mac_and_nowhere_else() {
        assert!(ctrl_held(false, true, true));
        // On Linux the same key is the Super key, which the desktop owns — it must not save a file.
        assert!(!ctrl_held(false, true, false));
        // Physical Ctrl is still Ctrl on a Mac: Vim's ctrl-r and the shell's ctrl-c live there.
        assert!(ctrl_held(true, false, true));
        assert!(!ctrl_held(false, false, true));
    }

    #[test]
    fn the_option_key_named_is_alt_and_the_other_still_types() {
        use OptionKey::*;
        // Left Option as Alt: `alt-f` works from the left, and the right types `ƒ` or `@`.
        assert!(alt_held(true, true, false, false, Left, true));
        assert!(!alt_held(true, false, true, false, Left, true));
        assert!(alt_held(true, false, true, false, Right, true));
        assert!(alt_held(true, false, true, false, Both, true));
        assert!(!alt_held(true, true, false, false, Neither, true));
        // With ⌘ down nothing is typed, so either side is ⌥ — ⌥⌘F is replace from either hand.
        assert!(alt_held(true, false, true, true, Left, true));
        assert!(alt_held(true, true, false, true, Neither, true));
        // Off a Mac the setting means nothing.
        assert!(alt_held(true, false, false, false, Neither, false));
        assert!(!alt_held(false, false, false, false, Both, false));
        // A platform that reports Alt but not its side is believed, unless Alt is off entirely.
        assert!(alt_held(true, false, false, false, Right, true));
        assert!(!alt_held(true, false, false, false, Neither, true));
        for key in OptionKey::ALL {
            assert_eq!(OptionKey::from_name(key.name()), Some(key));
        }
        assert_eq!(
            OptionKey::ALL.map(OptionKey::name),
            dacode_lua::OPTION_AS_ALT,
            "the config reader accepts exactly these"
        );
    }

    #[test]
    fn the_login_path_is_read_from_between_the_marks_whatever_the_profile_prints() {
        let printed = format!(
            "Welcome back!\nnvm: using node v22\n{PATH_MARK}\n/Users/ada/.nvm/versions/node/v22/bin:/usr/bin\n{PATH_MARK}\n"
        );
        assert_eq!(
            parse_login_path(&printed),
            Some(OsString::from(
                "/Users/ada/.nvm/versions/node/v22/bin:/usr/bin"
            ))
        );
        // A shell that died before the second mark printed a path it may not have finished.
        assert_eq!(parse_login_path(&format!("{PATH_MARK}\n/usr/bin")), None);
        assert_eq!(parse_login_path("no marks at all"), None);
        assert_eq!(
            parse_login_path(&format!("{PATH_MARK}\n\n{PATH_MARK}")),
            None
        );
    }

    #[test]
    fn only_a_launch_from_outside_a_terminal_asks_the_shell() {
        assert!(wants_login_shell(None, true));
        assert!(wants_login_shell(Some(OsStr::new("")), true));
        assert!(!wants_login_shell(Some(OsStr::new("xterm-256color")), true));
        assert!(!wants_login_shell(None, false));
    }

    #[test]
    fn a_real_shell_answers_with_its_path_and_a_stuck_one_is_cut_off() {
        // `sh` is what every Unix guarantees (see CLAUDE.md on naming programs in tests); Windows
        // has no login shell to ask.
        if cfg!(windows) {
            return;
        }
        let path = login_shell_path(Path::new("/bin/sh"), LOGIN_SHELL_TIMEOUT);
        assert!(path.is_some_and(|path| !path.is_empty()));

        // A "shell" that never answers must cost the timeout and no more.
        let dir = std::env::temp_dir().join(format!("dacode-stuck-shell-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stuck = dir.join("stuck");
        std::fs::write(&stuck, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let started = Instant::now();
        assert_eq!(login_shell_path(&stuck, Duration::from_millis(200)), None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_the_finder_hands_over_are_kept_in_order_until_taken_once() {
        // One test owns the queue: it is process-wide, as AppKit's call into it is.
        files_opened([PathBuf::from("/a.rs"), PathBuf::from("/b.rs")]);
        files_opened([PathBuf::from("/project")]);
        assert_eq!(
            take_opened_files(),
            [
                PathBuf::from("/a.rs"),
                PathBuf::from("/b.rs"),
                PathBuf::from("/project")
            ]
        );
        assert!(take_opened_files().is_empty(), "a file is opened once");
    }

    #[test]
    fn command_h_hides_and_command_m_minimises_but_ctrl_h_is_still_replace() {
        assert_eq!(
            window_key('h', true, false, false, false),
            Some(WindowKey::Hide)
        );
        assert_eq!(
            window_key('H', true, false, false, false),
            Some(WindowKey::Hide)
        );
        assert_eq!(
            window_key('h', true, false, true, false),
            Some(WindowKey::HideOthers)
        );
        assert_eq!(
            window_key('m', true, false, false, false),
            Some(WindowKey::Minimise)
        );
        // Physical Ctrl, alone or with ⌘, belongs to the editor.
        assert_eq!(window_key('h', false, true, false, false), None);
        assert_eq!(window_key('h', true, true, false, false), None);
        // ⇧⌘H and ⇧⌘M are ordinary bindings; only the bare chords are the Mac's.
        assert_eq!(window_key('m', true, false, false, true), None);
        assert_eq!(window_key('s', true, false, false, false), None);
        // The table must agree, or a menu prints a key that hides the window.
        for reserved in dacode_input::commands::MAC_RESERVED {
            let chord = reserved.trim_start_matches('<').trim_end_matches('>');
            let option = chord.contains("a-");
            let character = chord.chars().last().unwrap();
            assert!(
                window_key(character, true, false, option, false).is_some(),
                "{reserved} is reserved but not handled"
            );
        }
    }

    #[test]
    fn a_finder_launch_gets_the_directories_a_login_shell_would_have() {
        let launchd = OsString::from("/usr/bin:/bin:/usr/sbin:/sbin");
        let etc = vec![
            "/usr/local/bin\n/usr/bin\n/bin\n/usr/sbin\n/sbin\n".to_string(),
            "# a comment\n/Library/Apple/usr/bin\n\n".to_string(),
        ];
        let path = finder_path(Some(&launchd), Path::new("/Users/ada"), &etc).unwrap();
        let parts: Vec<PathBuf> = std::env::split_paths(&path).collect();
        let want: Vec<PathBuf> = [
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
            "/usr/local/bin",
            "/Library/Apple/usr/bin",
            "/opt/homebrew/bin",
            "/opt/homebrew/sbin",
            "/Users/ada/.cargo/bin",
            "/Users/ada/.local/bin",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        // What launchd gave comes first and keeps its order: a directory added here must never
        // shadow one the system already chose.
        assert_eq!(parts, want);
    }

    #[test]
    fn a_terminal_launch_that_already_has_everything_is_left_alone() {
        // Started from a shell, PATH is already the login one; rewriting it would be a no-op at
        // best and a reordering at worst.
        let full = std::env::join_paths(
            [
                "/opt/homebrew/bin",
                "/opt/homebrew/sbin",
                "/usr/local/bin",
                "/usr/bin",
                "/Users/ada/.cargo/bin",
                "/Users/ada/.local/bin",
            ]
            .iter(),
        )
        .unwrap();
        assert_eq!(finder_path(Some(&full), Path::new("/Users/ada"), &[]), None);
    }

    #[test]
    fn no_home_adds_nothing_at_the_root() {
        let path = finder_path(None, Path::new(""), &[]).unwrap();
        let parts: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert!(
            parts.iter().all(|part| !part.starts_with("/.cargo")),
            "{parts:?}"
        );
    }
}

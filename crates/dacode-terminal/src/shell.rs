//! Which shell the terminal starts.
//!
//! The platform and the environment are parameters, as they are in `dacode::paths::resolve`, so the
//! Windows rules have tests that run on Linux. A `cfg!(windows)` inside the function would make the
//! half of it that nobody here can run the half that nobody tests.

/// The two families of platform the choice differs between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Platform {
    /// The platform this build is for.
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }
}

/// A program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ShellCommand {
    pub fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }
}

/// The shells to try, in order. The terminal starts the first one that spawns.
///
/// **Unix:** `$SHELL`, which is the user's own answer to the question, and then `/bin/sh`, which
/// POSIX guarantees exists. Not a login shell (`-l`): the editor was itself started from a session
/// that already ran the login files, and running them again per terminal is what makes a terminal
/// slow to open for anyone with a heavy profile.
///
/// **Windows:** PowerShell, which every supported Windows ships and which is what VSCode opens,
/// then `%COMSPEC%` — `cmd.exe`, the one shell Windows is guaranteed to name for you — and `cmd.exe`
/// itself if even that is unset. `-NoLogo` because a copyright banner on every new terminal is noise.
pub fn shells_for(platform: Platform, var: impl Fn(&str) -> Option<String>) -> Vec<ShellCommand> {
    let set = |name: &str| var(name).filter(|value| !value.trim().is_empty());
    match platform {
        Platform::Unix => {
            let mut shells = Vec::new();
            if let Some(shell) = set("SHELL") {
                shells.push(ShellCommand::new(shell, &[]));
            }
            if shells.iter().all(|shell| shell.program != "/bin/sh") {
                shells.push(ShellCommand::new("/bin/sh", &[]));
            }
            shells
        }
        Platform::Windows => {
            let mut shells = vec![ShellCommand::new("powershell.exe", &["-NoLogo"])];
            match set("COMSPEC") {
                Some(comspec) => shells.push(ShellCommand::new(comspec, &[])),
                None => shells.push(ShellCommand::new("cmd.exe", &[])),
            }
            shells
        }
    }
}

/// The variables the shell is given on top of the editor's own environment.
///
/// `TERM` names what the parser understands: xterm's vocabulary with 256 colours, which every
/// program that draws in a terminal knows. `COLORTERM` is how a program learns that 24-bit colour
/// works too. Passed to the child rather than set on this process — alacritty's own `setup_env`
/// mutates the process environment, which is not thread-safe against every other thread reading it
/// and once deadlocked this test suite inside glibc.
pub fn environment() -> Vec<(String, String)> {
    [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("TERM_PROGRAM", "dacode"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn unix_starts_the_users_shell_and_falls_back_to_sh() {
        let shells = shells_for(Platform::Unix, env(&[("SHELL", "/usr/bin/fish")]));
        assert_eq!(
            shells,
            vec![
                ShellCommand::new("/usr/bin/fish", &[]),
                ShellCommand::new("/bin/sh", &[])
            ]
        );
    }

    #[test]
    fn unix_without_shell_set_still_has_a_shell() {
        // A desktop launcher can start the editor with an environment that names no shell at all.
        // A terminal that opened onto nothing would read as broken; `/bin/sh` always exists.
        for shells in [
            shells_for(Platform::Unix, env(&[])),
            shells_for(Platform::Unix, env(&[("SHELL", "  ")])),
        ] {
            assert_eq!(shells, vec![ShellCommand::new("/bin/sh", &[])]);
        }
    }

    #[test]
    fn sh_is_not_tried_twice() {
        let shells = shells_for(Platform::Unix, env(&[("SHELL", "/bin/sh")]));
        assert_eq!(shells.len(), 1);
    }

    #[test]
    fn windows_prefers_powershell_then_comspec() {
        let shells = shells_for(
            Platform::Windows,
            env(&[
                ("COMSPEC", r"C:\Windows\system32\cmd.exe"),
                // A Unix variable that leaks in through Git Bash must not decide the Windows shell:
                // it names a path in an MSYS filesystem that ConPTY cannot start.
                ("SHELL", "/usr/bin/bash"),
            ]),
        );
        assert_eq!(
            shells,
            vec![
                ShellCommand::new("powershell.exe", &["-NoLogo"]),
                ShellCommand::new(r"C:\Windows\system32\cmd.exe", &[]),
            ]
        );
    }

    #[test]
    fn windows_without_comspec_falls_back_to_cmd() {
        let shells = shells_for(Platform::Windows, env(&[]));
        assert_eq!(shells[1], ShellCommand::new("cmd.exe", &[]));
    }

    #[test]
    fn the_shell_is_told_what_it_is_talking_to() {
        let vars = environment();
        assert!(vars.contains(&("TERM".to_string(), "xterm-256color".to_string())));
        assert!(vars.contains(&("COLORTERM".to_string(), "truecolor".to_string())));
    }
}

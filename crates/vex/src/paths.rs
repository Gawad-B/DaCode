//! Where vex keeps things, on each platform it runs on.
//!
//! One module rather than a `HOME` lookup in six files. The rules differ per platform in ways that
//! are easy to get subtly wrong — Windows separates roaming configuration from machine-local state,
//! and putting a session file in the roaming directory means it follows the user to a machine where
//! those paths do not exist.
//!
//! Two kinds of directory, and the distinction matters:
//!
//! - **config** is the user's: `init.lua`, themes, snippets. They edit it, back it up, and on Windows
//!   it roams with the profile.
//! - **state** is the machine's: crash recovery copies and session files. They never edit it, and it
//!   must not roam — a recovery copy of a file that only exists on one computer is meaningless on
//!   another.

use std::path::PathBuf;

/// The user's configuration directory.
///
/// `VEX_CONFIG_DIR` overrides everything, which is what lets tests and trial runs use their own
/// without touching the real one.
pub fn config_dir() -> PathBuf {
    resolve(
        Where::Config,
        cfg!(windows),
        &Environment {
            explicit: var("VEX_CONFIG_DIR"),
            appdata: var("APPDATA"),
            local_appdata: var("LOCALAPPDATA"),
            xdg_config: var("XDG_CONFIG_HOME"),
            xdg_state: var("XDG_STATE_HOME"),
            home: home(),
        },
    )
}

/// Which directory is being asked for.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Where {
    Config,
    State,
}

/// The environment variables the answer depends on.
///
/// Passed in rather than read inside, so the rules can be tested — including the **Windows** rules,
/// from a Linux machine, which a `cfg!(windows)` branch cannot be. An earlier version set and unset
/// real environment variables in its tests; `setenv` is not thread-safe against the `getenv` that a
/// hundred other tests were doing in parallel, and the suite hung in glibc for twenty minutes. The
/// same lesson the recovery directory taught: make it a parameter.
#[derive(Debug, Default, Clone)]
pub struct Environment {
    pub explicit: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub xdg_state: Option<PathBuf>,
    pub home: PathBuf,
}

/// Where a directory goes, given the platform and the environment.
pub fn resolve(which: Where, windows: bool, environment: &Environment) -> PathBuf {
    if let Some(explicit) = &environment.explicit {
        return explicit.clone();
    }

    if windows {
        // Configuration goes in the roaming profile, because it is the user's and follows them.
        // State goes in the *local* one, because it must not: a recovery copy of a file that exists
        // on one machine is meaningless on another.
        let base = match which {
            Where::Config => environment.appdata.as_ref(),
            Where::State => environment.local_appdata.as_ref(),
        };
        if let Some(base) = base {
            return base.join("vex");
        }
        return environment.home.join("vex");
    }

    let xdg = match which {
        Where::Config => environment.xdg_config.as_ref(),
        Where::State => environment.xdg_state.as_ref(),
    };
    if let Some(xdg) = xdg {
        return xdg.join("vex");
    }

    environment.home.join(match which {
        Where::Config => ".config/vex",
        Where::State => ".local/state/vex",
    })
}

fn var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The machine-local state directory: recovery copies and sessions.
pub fn state_dir() -> PathBuf {
    resolve(
        Where::State,
        cfg!(windows),
        &Environment {
            explicit: var("VEX_STATE_DIR"),
            appdata: var("APPDATA"),
            local_appdata: var("LOCALAPPDATA"),
            xdg_config: var("XDG_CONFIG_HOME"),
            xdg_state: var("XDG_STATE_HOME"),
            home: home(),
        },
    )
}

/// The user's home directory, by whichever name this platform gives it.
pub fn home() -> PathBuf {
    // `USERPROFILE` on Windows; `HOME` elsewhere, and also on Windows under a POSIX shell, where it
    // is set and correct.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Whether a program is on `PATH`.
///
/// On Windows an executable is `name.exe`, `name.cmd` or `name.bat` — a bare-name check finds none of
/// them, which is how a Windows build would report every language server as missing while they were
/// all installed. `PATHEXT` is consulted because it is the list the platform itself uses.
pub fn program_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };

    std::env::split_paths(&path).any(|directory| {
        if directory.join(program).is_file() {
            return true;
        }
        extensions
            .iter()
            .any(|extension| directory.join(format!("{program}.{extension}")).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix() -> Environment {
        Environment {
            home: PathBuf::from("/home/someone"),
            ..Default::default()
        }
    }

    fn windows() -> Environment {
        Environment {
            appdata: Some(PathBuf::from("C:/Users/someone/AppData/Roaming")),
            local_appdata: Some(PathBuf::from("C:/Users/someone/AppData/Local")),
            home: PathBuf::from("C:/Users/someone"),
            ..Default::default()
        }
    }

    #[test]
    fn unix_follows_the_xdg_layout() {
        assert_eq!(
            resolve(Where::Config, false, &unix()),
            PathBuf::from("/home/someone/.config/vex")
        );
        assert_eq!(
            resolve(Where::State, false, &unix()),
            PathBuf::from("/home/someone/.local/state/vex")
        );

        let with_xdg = Environment {
            xdg_config: Some(PathBuf::from("/elsewhere/config")),
            xdg_state: Some(PathBuf::from("/elsewhere/state")),
            ..unix()
        };
        assert_eq!(
            resolve(Where::Config, false, &with_xdg),
            PathBuf::from("/elsewhere/config/vex"),
            "someone who sets XDG_CONFIG_HOME means it"
        );
        assert_eq!(
            resolve(Where::State, false, &with_xdg),
            PathBuf::from("/elsewhere/state/vex")
        );
    }

    #[test]
    fn windows_separates_roaming_configuration_from_local_state() {
        // The rule that is easy to get wrong, and the reason this function takes its environment as
        // an argument: it can be checked from a Linux machine.
        assert_eq!(
            resolve(Where::Config, true, &windows()),
            PathBuf::from("C:/Users/someone/AppData/Roaming/vex"),
            "configuration roams with the profile"
        );
        assert_eq!(
            resolve(Where::State, true, &windows()),
            PathBuf::from("C:/Users/someone/AppData/Local/vex"),
            "state must not roam: a recovery copy of a local file is meaningless elsewhere"
        );
    }

    #[test]
    fn windows_falls_back_to_the_profile_when_appdata_is_missing() {
        let bare = Environment {
            home: PathBuf::from("C:/Users/someone"),
            ..Default::default()
        };
        assert_eq!(
            resolve(Where::Config, true, &bare),
            PathBuf::from("C:/Users/someone/vex"),
            "somewhere writable beats nowhere"
        );
    }

    #[test]
    fn the_override_wins_on_every_platform() {
        // Tests and trial runs depend on this: without it they would write into the real config.
        for windows in [true, false] {
            let overridden = Environment {
                explicit: Some(PathBuf::from("/tmp/vex-elsewhere")),
                ..windows_or_unix(windows)
            };
            assert_eq!(
                resolve(Where::Config, windows, &overridden),
                PathBuf::from("/tmp/vex-elsewhere")
            );
        }
    }

    #[test]
    fn config_and_state_never_share_a_directory() {
        for windows in [true, false] {
            let environment = windows_or_unix(windows);
            assert_ne!(
                resolve(Where::Config, windows, &environment),
                resolve(Where::State, windows, &environment),
                "configuration the user edits and state they never see must not share a home"
            );
        }
    }

    #[test]
    fn an_empty_variable_is_treated_as_unset() {
        // `APPDATA=` set to nothing is how a stripped environment presents, and joining onto it would
        // put the config directory at the filesystem root.
        let empty = Environment {
            appdata: None,
            ..windows()
        };
        assert_eq!(
            resolve(Where::Config, true, &empty),
            PathBuf::from("C:/Users/someone/vex")
        );
    }

    #[test]
    fn a_program_that_exists_is_found_and_one_that_does_not_is_not() {
        let known = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(program_on_path(known), "{known} should be on PATH");
        assert!(!program_on_path("definitely-not-a-real-program-xyzzy"));
    }

    fn windows_or_unix(windows: bool) -> Environment {
        if windows { self::windows() } else { unix() }
    }
}

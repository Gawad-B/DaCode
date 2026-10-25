//! Where DaCode keeps things, on each platform it runs on.
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

use std::path::{Path, PathBuf};

/// The name the directories had before the editor was called DaCode.
const LEGACY_NAME: &str = "vex";

/// The user's configuration directory.
///
/// `DACODE_CONFIG_DIR` overrides everything, which is what lets tests and trial runs use their own
/// without touching the real one.
pub fn config_dir() -> PathBuf {
    resolve(Where::Config, cfg!(windows), &environment(Where::Config))
}

/// The environment as it is, for [`resolve`].
///
/// `VEX_CONFIG_DIR` and `VEX_STATE_DIR` are still read, after the new names, so a script or a
/// desktop entry written for vex keeps pointing where it did.
fn environment(which: Where) -> Environment {
    let (current, legacy) = match which {
        Where::Config => ("DACODE_CONFIG_DIR", "VEX_CONFIG_DIR"),
        Where::State => ("DACODE_STATE_DIR", "VEX_STATE_DIR"),
    };
    Environment {
        explicit: var(current).or_else(|| var(legacy)),
        appdata: var("APPDATA"),
        local_appdata: var("LOCALAPPDATA"),
        xdg_config: var("XDG_CONFIG_HOME"),
        xdg_state: var("XDG_STATE_HOME"),
        home: home(),
    }
}

/// Copies a vex directory to where DaCode looks, the first time DaCode looks there.
///
/// Copied, not moved: an older vex still installed beside DaCode keeps its configuration, and a
/// copy can be deleted where a move cannot be undone. Only when the new directory does not exist at
/// all, so it runs once and never overwrites anything DaCode has written. An explicit directory is
/// left alone — whoever set one chose where configuration lives.
///
/// Returns the directory it copied from, if it did.
pub fn migrate(which: Where, windows: bool, environment: &Environment) -> Option<PathBuf> {
    if environment.explicit.is_some() {
        return None;
    }
    let new = resolve(which, windows, environment);
    let old = resolve_named(which, windows, environment, LEGACY_NAME);
    if new.exists() || !old.is_dir() {
        return None;
    }
    copy_tree(&old, &new).ok()?;
    Some(old)
}

/// Both directories, for `main` to call once at startup.
pub fn migrate_from_vex() {
    for which in [Where::Config, Where::State] {
        if let Some(old) = migrate(which, cfg!(windows), &environment(which)) {
            eprintln!(
                "dacode: copied {} to {}",
                old.display(),
                resolve(which, cfg!(windows), &environment(which)).display()
            );
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        // `file_type` does not follow links, so a symlinked init.lua — a dotfiles checkout — is
        // copied as the link it is rather than flattened into a file that stops tracking the repo.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_symlink() && !cfg!(windows) {
            copy_link(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::copy(from, to).map(|_| ())
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
    resolve_named(which, windows, environment, "dacode")
}

fn resolve_named(which: Where, windows: bool, environment: &Environment, name: &str) -> PathBuf {
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
            return base.join(name);
        }
        return environment.home.join(name);
    }

    let xdg = match which {
        Where::Config => environment.xdg_config.as_ref(),
        Where::State => environment.xdg_state.as_ref(),
    };
    if let Some(xdg) = xdg {
        return xdg.join(name);
    }

    environment.home.join(match which {
        Where::Config => Path::new(".config").join(name),
        Where::State => Path::new(".local/state").join(name),
    })
}

fn var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The machine-local state directory: recovery copies and sessions.
pub fn state_dir() -> PathBuf {
    resolve(Where::State, cfg!(windows), &environment(Where::State))
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

    fn rooted(home: &Path) -> Environment {
        Environment {
            home: home.to_path_buf(),
            ..Default::default()
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dacode-paths-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_vex_configuration_is_copied_to_where_dacode_looks() {
        // The rename moved every directory. Without this, someone upgrading opened an editor that
        // had forgotten their keymap, their theme and the first-run answers — and so asked them again.
        let home = scratch("migrate");
        let old = home.join(".config/vex");
        std::fs::create_dir_all(old.join("themes")).unwrap();
        std::fs::write(old.join("init.lua"), "vex.set { tab_width = 2 }\n").unwrap();
        std::fs::write(old.join("themes/mine.toml"), "x\n").unwrap();

        let copied = migrate(Where::Config, false, &rooted(&home));

        assert_eq!(copied, Some(old.clone()));
        let new = home.join(".config/dacode");
        assert_eq!(
            std::fs::read_to_string(new.join("init.lua")).unwrap(),
            "vex.set { tab_width = 2 }\n"
        );
        assert!(
            new.join("themes/mine.toml").exists(),
            "subdirectories come too"
        );
        assert!(
            old.join("init.lua").exists(),
            "copied, so an old vex keeps working"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn migration_never_overwrites_what_dacode_already_has() {
        let home = scratch("no-overwrite");
        std::fs::create_dir_all(home.join(".config/vex")).unwrap();
        std::fs::write(home.join(".config/vex/init.lua"), "old\n").unwrap();
        std::fs::create_dir_all(home.join(".config/dacode")).unwrap();
        std::fs::write(home.join(".config/dacode/init.lua"), "new\n").unwrap();

        assert_eq!(migrate(Where::Config, false, &rooted(&home)), None);
        assert_eq!(
            std::fs::read_to_string(home.join(".config/dacode/init.lua")).unwrap(),
            "new\n"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_explicit_directory_is_never_migrated_into() {
        let home = scratch("explicit");
        std::fs::create_dir_all(home.join(".config/vex")).unwrap();
        let environment = Environment {
            explicit: Some(home.join("elsewhere")),
            ..rooted(&home)
        };
        assert_eq!(migrate(Where::Config, false, &environment), None);
        assert!(!home.join("elsewhere").exists());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_old_directories_are_found_on_every_platform() {
        let unix = rooted(Path::new("/home/someone"));
        assert_eq!(
            resolve_named(Where::State, false, &unix, LEGACY_NAME),
            PathBuf::from("/home/someone/.local/state/vex")
        );
        let windows = Environment {
            local_appdata: Some(PathBuf::from("C:/Users/someone/AppData/Local")),
            ..Default::default()
        };
        assert_eq!(
            resolve_named(Where::State, true, &windows, LEGACY_NAME),
            PathBuf::from("C:/Users/someone/AppData/Local/vex")
        );
    }

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
            PathBuf::from("/home/someone/.config/dacode")
        );
        assert_eq!(
            resolve(Where::State, false, &unix()),
            PathBuf::from("/home/someone/.local/state/dacode")
        );

        let with_xdg = Environment {
            xdg_config: Some(PathBuf::from("/elsewhere/config")),
            xdg_state: Some(PathBuf::from("/elsewhere/state")),
            ..unix()
        };
        assert_eq!(
            resolve(Where::Config, false, &with_xdg),
            PathBuf::from("/elsewhere/config/dacode"),
            "someone who sets XDG_CONFIG_HOME means it"
        );
        assert_eq!(
            resolve(Where::State, false, &with_xdg),
            PathBuf::from("/elsewhere/state/dacode")
        );
    }

    #[test]
    fn windows_separates_roaming_configuration_from_local_state() {
        // The rule that is easy to get wrong, and the reason this function takes its environment as
        // an argument: it can be checked from a Linux machine.
        assert_eq!(
            resolve(Where::Config, true, &windows()),
            PathBuf::from("C:/Users/someone/AppData/Roaming/dacode"),
            "configuration roams with the profile"
        );
        assert_eq!(
            resolve(Where::State, true, &windows()),
            PathBuf::from("C:/Users/someone/AppData/Local/dacode"),
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
            PathBuf::from("C:/Users/someone/dacode"),
            "somewhere writable beats nowhere"
        );
    }

    #[test]
    fn the_override_wins_on_every_platform() {
        // Tests and trial runs depend on this: without it they would write into the real config.
        for windows in [true, false] {
            let overridden = Environment {
                explicit: Some(PathBuf::from("/tmp/dacode-elsewhere")),
                ..windows_or_unix(windows)
            };
            assert_eq!(
                resolve(Where::Config, windows, &overridden),
                PathBuf::from("/tmp/dacode-elsewhere")
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
            PathBuf::from("C:/Users/someone/dacode")
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

//! Spawning helper programs without a console window appearing.
//!
//! The release build is a GUI application, which means it has no console of its own. A child that is
//! a *console* program — `git`, `powershell`, a language server — therefore gets a brand new one
//! allocated for it, and Windows shows that as a black window flashing on top of the editor. The
//! folder chooser looked like it was opening a terminal for exactly this reason, and the git gutter
//! would have flashed one on every reload.
//!
//! `CREATE_NO_WINDOW` is the flag that says "run it, give it no console". There is no cross-platform
//! way to set it, so this is the one place that reaches for `std::os::windows`.

use std::process::Command;

/// Runs `command` without giving it a console window. A no-op off Windows, where nothing allocates
/// one in the first place.
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW. Spelled out rather than pulled from `windows-sys` for one constant.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A [`Command`] that will not flash a console window. Prefer this to `Command::new` anywhere in the
/// editor — the CLI paths (`--import`, and anything else run from a terminal on purpose) are the
/// exception, and they already have a console to inherit.
pub fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    quiet(&mut command);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_command_still_runs_and_still_reports_its_output() {
        // The flag must not change what the program does or how its output is read — the whole point
        // is that only the console window differs. Checked with a program every platform has.
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c", "echo vex"])
        } else {
            ("echo", vec!["vex"])
        };

        let output = command(program).args(args).output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "vex");
    }
}

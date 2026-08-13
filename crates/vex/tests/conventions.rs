//! Conventions that a compiler cannot check but a repository can.
//!
//! Greps wearing tests' clothes, which is unusual — but the alternative is a rule written in
//! `CLAUDE.md` and then not followed, which is what happened with `canonicalize` three times running,
//! each costing a fifteen-minute CI round trip to discover. The second rule joined it when a GUI build
//! on Windows started flashing a console window every time it asked git anything.

use std::path::{Path, PathBuf};

/// The only places allowed to call `canonicalize` directly.
///
/// `vex-project` because that is where the wrapper lives, and `vex-lsp` because it must not depend on
/// the project layer to build a URI — an LSP client knows about paths, not projects.
const SANCTIONED: [&str; 3] = [
    "crates/vex-project/src/lib.rs",
    "crates/vex-lsp/src/position.rs",
    // This file states the rule, so it necessarily contains the thing it looks for. Exempted
    // explicitly rather than by building the needle at runtime, which would be cleverer and less
    // obvious to whoever reads this next.
    "crates/vex/tests/conventions.rs",
];

#[test]
fn paths_are_resolved_through_one_function() {
    // On Windows `canonicalize` returns `\\?\C:\x`, which is right for the filesystem API and wrong for
    // displaying, comparing and URI-building. Mixing the two forms produced three separate CI failures:
    // a URI rust-analyzer refused, a root compared against `temp_dir()`, and two tests comparing
    // against the unstripped form. `vex_project::canonical` is the one answer.
    let mut offenders = Vec::new();

    for file in rust_files(&workspace_root()) {
        let relative = file.to_string_lossy().replace('\\', "/");
        if SANCTIONED.iter().any(|allowed| relative.ends_with(allowed)) {
            continue;
        }

        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if line.contains(".canonicalize()") {
                offenders.push(format!("{relative}:{}: {}", number + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "use vex_project::canonical instead of canonicalize directly — on Windows the latter returns \
         an extended-length path that must not reach comparisons, URIs or the screen:\n{}",
        offenders.join("\n")
    );
}

/// The only places allowed to spawn a process with a bare `Command::new`.
const MAY_SPAWN: [&str; 5] = [
    // Where the helper lives, and its counterpart in vex-lsp.
    "crates/vex/src/process.rs",
    "crates/vex-lsp/src/client.rs",
    // Run from a terminal on purpose, where inheriting the console is what you want: the import CLI's
    // downloader and the language-server probe the tests use.
    "crates/vex-import/src/vsix.rs",
    "crates/vex-lsp/src/lib.rs",
    "crates/vex/tests/conventions.rs",
];

#[test]
fn helper_processes_are_spawned_without_a_console_window() {
    // The release build is a GUI application and so has no console. Windows gives a console child one
    // of its own, and shows it: a black window over the editor, once per git call. `process::command`
    // sets CREATE_NO_WINDOW; a bare `Command::new` does not, and nothing on Linux can see the
    // difference — which is exactly the kind of rule that needs a test rather than a paragraph.
    let mut offenders = Vec::new();

    for file in rust_files(&workspace_root()) {
        let relative = file.to_string_lossy().replace('\\', "/");
        if MAY_SPAWN.iter().any(|allowed| relative.ends_with(allowed)) {
            continue;
        }

        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Tests may spawn whatever they like: they run from a terminal that already has a console.
        let source = text.split("\nmod tests {").next().unwrap_or(&text).to_string();
        for (number, line) in source.lines().enumerate() {
            if line.contains("Command::new(") {
                offenders.push(format!("{relative}:{}: {}", number + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "spawn through vex::process::command, not Command::new — on Windows a GUI application's \
         console child gets a console window of its own, in front of the editor:\n{}",
        offenders.join("\n")
    );
}

/// The workspace root, from the manifest directory rather than the working directory.
///
/// `cargo test` runs with the *crate* as the working directory, so a relative path here found nothing
/// and the test passed while scanning zero files — which is worse than not having it. Found by
/// injecting a violation and watching it pass.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Every `.rs` file in the workspace's crates, sources and tests alike.
fn rust_files(from: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![from.join("crates")];
    assert!(
        stack[0].is_dir(),
        "cannot find the crates directory at {:?} — this test would otherwise scan nothing and pass",
        stack[0]
    );

    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target` can appear inside a crate directory and holds generated code.
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    found
}

//! Conventions that a compiler cannot check but a repository can.
//!
//! One test, for one rule that has now broken a Windows build three times in a row. It is a grep
//! wearing a test's clothes, which is unusual — but the alternative is a rule written in `CLAUDE.md`
//! that I have demonstrably failed to follow three times, and each failure cost a fifteen-minute CI
//! round trip to discover.

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

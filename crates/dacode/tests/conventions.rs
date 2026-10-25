//! Conventions that a compiler cannot check but a repository can.
//!
//! Greps wearing tests' clothes, which is unusual — but the alternative is a rule written in
//! `CLAUDE.md` and then not followed, which is what happened with `canonicalize` three times running,
//! each costing a fifteen-minute CI round trip to discover. The second rule joined it when a GUI build
//! on Windows started flashing a console window every time it asked git anything.

use std::path::{Path, PathBuf};

/// The only places allowed to call `canonicalize` directly.
///
/// `dacode-project` because that is where the wrapper lives, and `dacode-lsp` because it must not depend on
/// the project layer to build a URI — an LSP client knows about paths, not projects.
const SANCTIONED: [&str; 3] = [
    "crates/dacode-project/src/lib.rs",
    "crates/dacode-lsp/src/position.rs",
    // This file states the rule, so it necessarily contains the thing it looks for. Exempted
    // explicitly rather than by building the needle at runtime, which would be cleverer and less
    // obvious to whoever reads this next.
    "crates/dacode/tests/conventions.rs",
];

#[test]
fn paths_are_resolved_through_one_function() {
    // On Windows `canonicalize` returns `\\?\C:\x`, which is right for the filesystem API and wrong for
    // displaying, comparing and URI-building. Mixing the two forms produced three separate CI failures:
    // a URI rust-analyzer refused, a root compared against `temp_dir()`, and two tests comparing
    // against the unstripped form. `dacode_project::canonical` is the one answer.
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
        "use dacode_project::canonical instead of canonicalize directly — on Windows the latter returns \
         an extended-length path that must not reach comparisons, URIs or the screen:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn nothing_modal_reaches_a_non_modal_reader() {
    // The rule the whole `flat`/`modal_only` split exists for, tested at the readers rather than at
    // the table — because the table was always right and it was a reader that leaked. With Vim off
    // the palette listed "start selecting", and choosing it put a non-modal editor into visual mode
    // with no way out that the user's own grammar could type.
    use dacode_input::commands;

    for command in commands::available(false) {
        assert!(
            !command.modal_only,
            "`{}` is modal-only and reached the non-modal command list",
            command.name
        );
    }

    // The help panel filters on having a key, which reaches the same place by another route.
    for line in dacode::help::Help::new(false).lines() {
        if let dacode::help::Line::Binding { keys, what } = line {
            assert!(!keys.is_empty(), "`{what}` is offered with no key to press");
        }
    }

    // And the which-key popup, whose two Vim-only prefixes `feed_flat` cannot even produce.
    for prefix in ["\"", "q", "g", "z"] {
        assert!(
            commands::continuations(prefix, false).is_empty(),
            "the which-key popup offers `{prefix}` continuations with the grammar off"
        );
    }
}

#[test]
fn no_ex_command_is_shown_without_asking_which_grammar_is_on() {
    // Four messages told everyone to press `:q!`, `:wq`, `:w <path>` and `:e` — including the people
    // who had turned Vim off, whose grammar cannot parse the leading `:`. Each is now behind a
    // `state.modal` branch. This is the grep that keeps the fifth one from being written: an ex
    // command may appear in a source line only where `modal` appears too.
    let mut offenders = Vec::new();

    for file in rust_files(&workspace_root()) {
        let relative = file.to_string_lossy().replace('\\', "/");
        if relative.ends_with("crates/dacode/tests/conventions.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };

        let lines: Vec<&str> = text.lines().collect();
        for (number, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            // Comments explain the history and are allowed to quote it.
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            let quotes_an_ex_command = [":q!", ":wq", ":w <path>", ":e to", ":w to"]
                .iter()
                .any(|needle| line.contains(needle));
            if !quotes_an_ex_command {
                continue;
            }

            // The branch is a few lines up — `if self.state.modal {` and then the string, or a
            // multi-line format! whose second half carries the needle. A window rather than the one
            // line, because what matters is that the decision was made nearby, not on which line.
            const LOOKBACK: usize = 5;
            let guarded = lines[number.saturating_sub(LOOKBACK)..=number]
                .iter()
                .any(|nearby| nearby.contains("modal"));
            if !guarded {
                offenders.push(format!("{relative}:{}: {}", number + 1, trimmed));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "an ex command is shown without checking whether the user has Vim's grammar on:\n{}",
        offenders.join("\n")
    );
}

/// The only places allowed to spawn a process with a bare `Command::new`.
const MAY_SPAWN: [&str; 5] = [
    // Where the helper lives, and its counterpart in dacode-lsp.
    "crates/dacode/src/process.rs",
    "crates/dacode-lsp/src/client.rs",
    // Run from a terminal on purpose, where inheriting the console is what you want: the import CLI's
    // downloader and the language-server probe the tests use.
    "crates/dacode-import/src/vsix.rs",
    "crates/dacode-lsp/src/lib.rs",
    "crates/dacode/tests/conventions.rs",
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
        let source = text
            .split("\nmod tests {")
            .next()
            .unwrap_or(&text)
            .to_string();
        for (number, line) in source.lines().enumerate() {
            if line.contains("Command::new(") {
                offenders.push(format!("{relative}:{}: {}", number + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "spawn through dacode::process::command, not Command::new — on Windows a GUI application's \
         console child gets a console window of its own, in front of the editor:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn a_buffer_is_saved_through_one_path() {
    // There were two. `:w` and autosave went through the app's `write_active`, which clears the
    // recovery copy, tells the watcher the write was its own and starts a format; `ctrl-s` reached
    // the executor's own `Save` arm, which wrote the file and did none of that. Nothing showed it —
    // the file was saved either way — until a crash copy was offered for work that had been saved.
    //
    // Marking a document saved is the one thing every save path must do, so it is what is counted:
    // a second writer has to call it too, and then this fails.
    let mut sites = Vec::new();

    for file in rust_files(&workspace_root()) {
        let relative = file.to_string_lossy().replace('\\', "/");
        if !relative.contains("/dacode/src/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Every test module in the binary sits below its first `#[cfg(test)]`, and they mark
        // documents saved freely to set up what they check.
        let source = text.split("\n#[cfg(test)]").next().unwrap_or(&text);
        for (number, line) in source.lines().enumerate() {
            if line.contains(".mark_saved()") && !line.trim_start().starts_with("//") {
                sites.push(format!("{relative}:{}: {}", number + 1, line.trim()));
            }
        }
    }

    assert!(
        sites.len() == 1 && sites[0].contains("crates/dacode/src/main.rs"),
        "a buffer must be saved only by `App::write_active`, which is what clears its recovery copy \
         and checks nobody else changed the file — found:\n{}",
        sites.join("\n")
    );
}

/// Every `Document` method that changes the text, and so moves the lines folds are named by.
const MUTATES_TEXT: [&str; 10] = [
    "apply",
    "insert",
    "insert_newline",
    "insert_newline_smart",
    "type_char",
    "delete_forward",
    "delete_backward",
    "delete_backward_pairs",
    "undo",
    "redo",
];

#[test]
fn text_is_changed_only_through_the_state_that_owns_the_folds() {
    // `Document::apply` is the single funnel for mutation, but the folds live outside the document,
    // in the display map. Nothing moved them, so they went on naming lines an edit had shifted or
    // deleted — and since a fold hides every line inside its range, one whose end had run off the
    // document hid *everything below its head*. Deleting a function above a closed fold made the
    // rest of the file vanish.
    //
    // `EditorState::mutate` replays the change queue over the folds afterwards. This is the grep
    // that keeps the next mutation site from going straight to the document and quietly skipping
    // it, because "remember to call the other thing too" is not a rule that survives a year.
    let mut offenders = Vec::new();
    let root = workspace_root();

    for file in rust_files(&root) {
        let relative = file.to_string_lossy().replace('\\', "/");
        // Only the binary has an `EditorState` to go through. Everything below it — and the tests
        // that drive a bare `Document` — legitimately holds the document directly.
        if !relative.contains("/dacode/src/") || relative.ends_with("conventions.rs") {
            continue;
        }

        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Tests drive a bare `Document` on purpose — they are checking the document, not the
        // editor around it.
        let source = text.split("\nmod tests {").next().unwrap_or(&text);
        let lines: Vec<&str> = source.lines().collect();

        for (number, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }

            // What is checked is the *receiver*, not a lookback window: inside a `mutate` closure
            // the document is named plainly, `document.insert(…)`, and everywhere else it is
            // reached through something — `state.document.insert(…)`. A window instead of this
            // read as passing while the injected violation sat two lines under an unrelated
            // `mutate` call, which is how a guard ends up checking nothing.
            let mutates_through_a_field = MUTATES_TEXT
                .iter()
                .any(|method| line.contains(&format!(".document.{method}(")));

            // The same call written over three lines, which rustfmt does whenever it is long
            // enough: `state`, then `.document`, then `.apply(…)`.
            let split_over_lines = trimmed == ".document"
                && lines.get(number + 1).is_some_and(|next| {
                    let next = next.trim_start();
                    MUTATES_TEXT
                        .iter()
                        .any(|method| next.starts_with(&format!(".{method}(")))
                });

            // Handing the document to something else that will change it — every one of these is
            // an `edits::` helper that builds its own transaction. Swapping a whole document is a
            // buffer change rather than an edit, and clears the folds on its own.
            let lent_out = (line.contains("&mut state.document")
                || line.contains("&mut self.state.document"))
                && !line.contains("mem::replace")
                && !line.contains("mem::swap");

            if mutates_through_a_field || split_over_lines || lent_out {
                offenders.push(format!("{relative}:{}: {}", number + 1, trimmed));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "change text through EditorState::mutate, not the document directly — the folds are line \
         numbers held outside the document and only that path moves them:\n{}",
        offenders.join("\n")
    );
}

/// The workspace root, from the manifest directory rather than the working directory.
///
/// `cargo test` runs with the *crate* as the working directory, so a relative path here found nothing
/// and the test passed while scanning zero files — which is worse than not having it. Found by
/// injecting a violation and watching it pass.
/// Every link in the documentation points at something that exists.
///
/// The documentation is now a tree rather than one file — a landing page, a tutorial, five guides,
/// four reference documents and an explanation — and the moment there is more than one file there are
/// relative links between them. A dead one is the cheapest possible defect to ship and among the most
/// irritating to meet: it costs nothing to write, nothing notices, and the person who finds it is
/// looking for the thing it was supposed to lead to.
///
/// Only local links. An external URL can rot too, but checking those needs the network, and a test
/// that fails when GitHub is slow is a test people learn to ignore.
#[test]
fn every_link_in_the_documentation_leads_somewhere() {
    let root = workspace_root();
    let mut broken = Vec::new();

    let files = markdown_files(&root);
    assert!(
        files.len() > 10,
        "found only {} markdown files — this test would otherwise check almost nothing and pass",
        files.len()
    );

    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let directory = file.parent().unwrap_or(&root);

        for target in links(&text) {
            // An anchor into the same page, an external URL, or a mail link: nothing on disk to
            // check. `//` catches protocol-relative URLs as well as `https://`.
            if target.starts_with('#')
                || target.starts_with("mailto:")
                || target.contains("://")
                || target.starts_with("//")
            {
                continue;
            }

            // `guides/keymaps.md#rebinding` names a file and a heading; only the file is ours to
            // verify, since a heading anchor is generated by whatever renders the page.
            let path = target.split('#').next().unwrap_or(&target);
            if path.is_empty() {
                continue;
            }

            if !directory.join(path).exists() {
                broken.push(format!("{relative} → {target}"));
            }
        }
    }

    assert!(
        broken.is_empty(),
        "documentation links pointing at nothing:\n  {}",
        broken.join("\n  ")
    );
}

/// The install commands in the documentation name the version that is actually being built.
///
/// `sudo apt install ./dacode_0.4.1_amd64.deb` is the most useful thing the README can say and the
/// first thing to go stale: a release bumps twelve `Cargo.toml` files and nothing anywhere points at
/// the sentence telling people what to download. A reader who copies it gets "no such file", which
/// reads as a broken project rather than an out-of-date page.
///
/// Only version numbers attached to the program's own name, so prose about what changed in an
/// earlier release — "run on Windows since 0.3.0" — is left alone. That distinction is the whole
/// reason this looks for `dacode_`, `dacode-` and `dacode ` rather than for any three numbers with dots.
#[test]
fn the_documented_install_commands_name_this_version() {
    let root = workspace_root();
    let expected = env!("CARGO_PKG_VERSION");
    let mut stale = Vec::new();

    for name in ["README.md", "docs/getting-started.md"] {
        let path = root.join(name);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("cannot read {name}, so this test checks nothing: {error}")
        });

        for separator in ['_', '-', ' '] {
            let needle = format!("dacode{separator}");
            for (index, _) in text.match_indices(&needle) {
                let rest = &text[index + needle.len()..];
                let version: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                // Two dots or it is not a version — `dacode-setup.exe` and `dacode-0.4.1` must not be
                // confused for one another, and neither must `x86_64`.
                if version.matches('.').count() == 2 && version != expected {
                    stale.push(format!("{name}: dacode{separator}{version}"));
                }
            }
        }
    }

    assert!(
        stale.is_empty(),
        "the documentation offers downloads for a version this tree is not:\n  {}\nthe crate says \
         {expected}",
        stale.join("\n  ")
    );
}

/// The landing page's download table names the version that is actually being built.
///
/// `web/src/useRelease.ts` used to fetch the real release from the GitHub API and fall back to a
/// hard-coded one; now that the repository is private, the table is all there is and every URL is
/// written by hand. That makes this guard matter more rather than less. It is the same hazard as
/// the README's install command, one directory further from anyone who would notice: nothing
/// fails, the button still renders, and it hands out the previous release.
#[test]
fn the_landing_page_release_names_this_version() {
    let root = workspace_root();
    let path = root.join("web/src/useRelease.ts");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read web/src/useRelease.ts: {error}"));

    let start = text
        .find("const RELEASE")
        .expect("no RELEASE in useRelease.ts — this test would check nothing");
    let block = &text[start..];
    let end = block
        .find("\n};")
        .expect("RELEASE is not closed by `};` — the scan would run to the end of the file");
    let block = &block[..end];

    // Every digits-dots-digits run inside the block. The asset sizes have no dots in them, so
    // anything with two is a version: the tag, the deb, the rpm and the tarball all carry one.
    let mut versions = Vec::new();
    let bytes: Vec<char> = block.chars().collect();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let from = index;
            while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == '.') {
                index += 1;
            }
            let run: String = bytes[from..index].iter().collect();
            if run.matches('.').count() == 2 {
                versions.push(run);
            }
        } else {
            index += 1;
        }
    }

    assert!(
        !versions.is_empty(),
        "found no version in the RELEASE block, so this test checks nothing"
    );

    let expected = env!("CARGO_PKG_VERSION");
    let stale: Vec<&String> = versions.iter().filter(|v| *v != expected).collect();
    assert!(
        stale.is_empty(),
        "web/src/useRelease.ts offers downloads for a version this tree is not: {stale:?}\nthe \
         crate says {expected}"
    );
}

/// The target of every markdown link and HTML `src` in `text`.
///
/// Deliberately crude — it does not parse markdown, it looks for `](` and `src="`. A reference-style
/// link or a link inside a fenced code block would be missed, and being missed is the right failure
/// for a check like this: the cost of a false negative is a link nobody verified, and the cost of a
/// false positive is a build that fails on prose.
fn links(text: &str) -> Vec<String> {
    let mut found = Vec::new();

    for (index, _) in text.match_indices("](") {
        if let Some(target) = text[index + 2..].split(')').next() {
            // `[text](path "title")` is legal; the title is not a path.
            found.push(target.split_whitespace().next().unwrap_or("").to_string());
        }
    }
    for pattern in ["src=\"", "srcset=\""] {
        for (index, _) in text.match_indices(pattern) {
            if let Some(target) = text[index + pattern.len()..].split('"').next() {
                found.push(target.to_string());
            }
        }
    }

    found.retain(|target| !target.is_empty());
    found
}

/// Every `.md` file that is part of the repository, generated and vendored trees aside.
fn markdown_files(from: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![from.to_path_buf()];

    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `node_modules` and `dist` belong to the landing page in `web/` and are not
                // ours: a dependency's README linking to a file it did not ship is that
                // dependency's problem, and there are thousands of them.
                let skip = path.file_name().is_some_and(|name| {
                    name == "target"
                        || name == ".git"
                        || name == "logo"
                        || name == "node_modules"
                        || name == "dist"
                });
                if !skip {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "md") {
                found.push(path);
            }
        }
    }

    found
}

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

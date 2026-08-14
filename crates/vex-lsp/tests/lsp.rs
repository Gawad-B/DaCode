//! Framing, position conversion, staleness — and a real handshake with rust-analyzer when it is
//! installed.

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use std::io::Cursor;
use vex_lsp::position::{path_to_uri, uri_to_path};
use vex_lsp::transport::{encode, read_message, write_message};
use vex_lsp::{Diagnostics, from_lsp, range_from_lsp, server_for, to_lsp};
use vex_text::{CharOffset, Text, Version};

#[test]
fn framing_round_trips() {
    let message = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
    let framed = encode(&message);

    let text = String::from_utf8_lossy(&framed);
    assert!(text.starts_with("Content-Length: "), "{text}");
    assert!(text.contains("\r\n\r\n"), "headers end with a blank line");

    let mut reader = Cursor::new(framed);
    let decoded = read_message(&mut reader).unwrap().unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn content_length_counts_bytes_not_characters() {
    // The one framing detail that breaks on non-ASCII: a length in characters desynchronizes the
    // stream permanently, and every later message is garbage.
    let message = serde_json::json!({ "text": "héllo 👋 日本語" });
    let framed = encode(&message);

    let header_end = framed
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("has a header terminator");
    let header = String::from_utf8_lossy(&framed[..header_end]);
    let declared: usize = header
        .trim_start_matches("Content-Length: ")
        .trim()
        .parse()
        .unwrap();

    assert_eq!(
        declared,
        framed.len() - header_end - 4,
        "declared length must match the byte count"
    );

    let mut reader = Cursor::new(framed);
    assert_eq!(read_message(&mut reader).unwrap().unwrap(), message);
}

#[test]
fn several_messages_read_back_in_order() {
    let mut stream = Vec::new();
    for id in 1..=3 {
        write_message(&mut stream, &serde_json::json!({ "id": id })).unwrap();
    }

    let mut reader = Cursor::new(stream);
    for id in 1..=3 {
        let message = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(message["id"], id);
    }
    assert!(
        read_message(&mut reader).unwrap().is_none(),
        "the end of the stream is not an error"
    );
}

#[test]
fn header_names_are_case_insensitive() {
    // Servers differ, and the spec says names are case-insensitive.
    let body = br#"{"id":7}"#;
    let mut stream = format!("content-length: {}\r\n\r\n", body.len()).into_bytes();
    stream.extend_from_slice(body);

    let mut reader = Cursor::new(stream);
    assert_eq!(read_message(&mut reader).unwrap().unwrap()["id"], 7);
}

#[test]
fn extra_headers_are_ignored() {
    let body = br#"{"id":8}"#;
    let mut stream = format!(
        "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    stream.extend_from_slice(body);

    let mut reader = Cursor::new(stream);
    assert_eq!(read_message(&mut reader).unwrap().unwrap()["id"], 8);
}

#[test]
fn a_missing_content_length_is_an_error_not_a_hang() {
    let mut reader = Cursor::new(b"X-Whatever: 1\r\n\r\n{}".to_vec());
    assert!(read_message(&mut reader).is_err());
}

#[test]
fn lsp_columns_are_utf16_code_units() {
    // The conversion the typed coordinate spaces exist for. An emoji is one char but two UTF-16
    // units; get this wrong and every position after it in the file is off by one.
    let text = Text::from_str("let a = \"👋\"; // done\n");

    let after_emoji = CharOffset(10);
    let position = to_lsp(&text, after_emoji);
    assert_eq!(position.line, 0);
    assert_eq!(
        position.character, 11,
        "9 chars before the emoji, plus 2 units for the emoji itself"
    );

    assert_eq!(
        from_lsp(&text, position),
        after_emoji,
        "and it must convert back"
    );
}

#[test]
fn positions_round_trip_on_every_line() {
    let text = Text::from_str("fn main() {\n    let x = \"héllo 👋\";\n}\n");

    for line in 0..=text.last_line() {
        for column in 0..=text.line_len(line) {
            let offset = text.point_to_char(vex_text::Point::new(line, column));
            let position = to_lsp(&text, offset);
            assert_eq!(
                from_lsp(&text, position),
                offset,
                "line {line} column {column} did not survive the round trip"
            );
        }
    }
}

#[test]
fn an_out_of_range_position_is_clamped_not_rejected() {
    // A server working from an older version will send positions past the end. Clamping keeps the
    // editor usable; the version check decides whether to trust the result at all.
    let text = Text::from_str("short\n");
    let far = Position {
        line: 99,
        character: 99,
    };
    assert!(from_lsp(&text, far) <= text.len_chars());
}

#[test]
fn inverted_ranges_are_ordered() {
    let text = Text::from_str("hello world\n");
    let range = range_from_lsp(
        &text,
        Range {
            start: Position {
                line: 0,
                character: 8,
            },
            end: Position {
                line: 0,
                character: 2,
            },
        },
    );
    assert!(range.start <= range.end, "consumers must not have to guard");
    assert_eq!(range.start.get(), 2);
    assert_eq!(range.end.get(), 8);
}

#[test]
fn uris_round_trip_including_spaces() {
    let path = std::path::Path::new("/tmp/some dir/file name.rs");
    let uri = path_to_uri(path);

    assert!(uri.starts_with("file://"), "{uri}");
    assert!(uri.contains("%20"), "spaces must be encoded: {uri}");
    assert_eq!(uri_to_path(&uri).unwrap(), path);
}

#[test]
fn a_non_file_uri_is_rejected() {
    assert!(uri_to_path("https://example.com/x.rs").is_none());
}

#[test]
fn stale_diagnostics_are_discarded() {
    // The rule from §1.1, applied to the service most likely to violate it: diagnostics arrive
    // hundreds of milliseconds late, by which time the text has moved.
    let mut diagnostics = Diagnostics::default();
    let item = Diagnostic {
        range: Range::default(),
        severity: Some(DiagnosticSeverity::ERROR),
        message: "boom".into(),
        ..Default::default()
    };

    assert!(
        diagnostics.apply(vec![item.clone()], Some(5), Version(5)),
        "a current result is applied"
    );
    assert_eq!(diagnostics.items().len(), 1);

    assert!(
        !diagnostics.apply(vec![], Some(4), Version(5)),
        "a result for an older version must be refused"
    );
    assert_eq!(
        diagnostics.items().len(),
        1,
        "and must not have replaced what was there"
    );

    assert!(
        diagnostics.apply(vec![item], None, Version(9)),
        "a server that reports no version gets the benefit of the doubt"
    );
}

#[test]
fn severities_are_counted_for_the_statusline() {
    let make = |severity| Diagnostic {
        range: Range::default(),
        severity: Some(severity),
        message: String::new(),
        ..Default::default()
    };

    let mut diagnostics = Diagnostics::default();
    diagnostics.apply(
        vec![
            make(DiagnosticSeverity::ERROR),
            make(DiagnosticSeverity::ERROR),
            make(DiagnosticSeverity::WARNING),
            make(DiagnosticSeverity::HINT),
        ],
        None,
        Version(1),
    );

    assert_eq!(diagnostics.counts(), (2, 1), "hints are not warnings");
}

#[test]
fn servers_are_chosen_by_extension() {
    let none = [];
    assert_eq!(
        server_for("src/main.rs", &none).unwrap().program,
        "rust-analyzer"
    );
    assert_eq!(server_for("app.py", &none).unwrap().language_id, "python");
    assert_eq!(server_for("main.cpp", &none).unwrap().program, "clangd");
    assert_eq!(server_for("notes.md", &none), None);

    // A dotfile has no extension. `rsplit('.')` said `.gitignore`'s was `gitignore`, which was
    // harmless while every comparison was against a fixed list and stops being harmless the moment
    // config can name extensions.
    assert_eq!(server_for(".gitignore", &none), None);
    assert_eq!(server_for("Makefile", &none), None);
}

#[test]
fn a_language_may_have_more_than_one_server() {
    // "The Python server" is not one program: pyright is an npm package and pylsp a pip one, and a
    // machine has whichever its owner installed. Offering only the first and then reporting it
    // missing is how an editor tells someone with a working language server that it has none.
    let none = [];
    let python = vex_lsp::servers_for("app.py", &none);
    assert!(python.len() > 1, "python should have alternatives");
    assert!(python.iter().all(|spec| spec.language == "Python"));
    assert!(
        python.iter().all(|spec| !spec.install.is_empty()),
        "every candidate has to be able to say how to install it"
    );
    assert!(vex_lsp::servers_for("notes.md", &none).is_empty());
}

#[test]
fn a_configured_server_is_tried_before_the_built_in_one() {
    // The case this exists for: a project with its own virtualenv. Naming it must not also throw
    // away the fallback, so the built-ins stay behind it rather than being replaced.
    let venv = vex_lsp::configure("python", ".venv/bin/pylsp", &[], &[]).expect("valid");
    assert_eq!(
        venv.extensions,
        vec!["py", "pyi"],
        "the extensions come from vex's own table, so config need not restate them"
    );
    assert_eq!(
        venv.spec.language_id, "python",
        "and so does the languageId"
    );

    let candidates = vex_lsp::servers_for("app.py", std::slice::from_ref(&venv));
    assert_eq!(candidates[0].program, ".venv/bin/pylsp");
    assert!(candidates[0].configured);
    assert!(
        candidates.len() > 1 && !candidates[1].configured,
        "the built-ins are still behind it: {candidates:?}"
    );

    // And it does not leak into another language.
    assert!(
        vex_lsp::servers_for("main.rs", std::slice::from_ref(&venv))
            .iter()
            .all(|spec| !spec.configured)
    );
}

#[test]
fn a_language_vex_has_never_heard_of_has_to_say_which_files_it_serves() {
    // A server that can never match a file is a line of config that silently does nothing, which is
    // the worst outcome available — so it is refused with the fix in the message.
    let refused = vex_lsp::configure("zig", "zls", &[], &[]).expect_err("no extensions");
    assert!(refused.contains("extensions"), "{refused}");
    assert!(refused.contains("zig"), "{refused}");

    let ok = vex_lsp::configure("zig", "zls", &[], &[".ZIG".to_string(), "zon".to_string()])
        .expect("with extensions");
    assert_eq!(
        ok.extensions,
        vec!["zig", "zon"],
        "a leading dot and capitals are what people write, and neither should matter"
    );
    assert_eq!(
        ok.spec.language_id, "zig",
        "an unknown language's id is what the user typed: they and their server are the only two \
         who know the name"
    );
    assert_eq!(
        vex_lsp::servers_for("build.zig", std::slice::from_ref(&ok)).len(),
        1
    );

    let empty = vex_lsp::configure("python", "  ", &[], &[]).expect_err("no command");
    assert!(empty.contains("command"), "{empty}");
}

#[test]
fn a_command_with_a_separator_in_it_is_a_path_not_a_name() {
    // The venv case. `PATH` is a list of directories, so looking a slash-containing string up in it
    // finds nothing, ever — which is how naming an interpreter's own server would have failed.
    let dir = std::env::temp_dir().join(format!("vex-command-{}", std::process::id()));
    let bin = dir.join(".venv").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("pylsp"), "").unwrap();
    std::fs::write(bin.join("other.exe"), "").unwrap();
    let empty = std::ffi::OsString::new();

    let resolve = |program: &str, windows: bool| {
        vex_lsp::resolve_command_in(program, &dir, None, &empty, Some(".EXE;.CMD"), windows)
    };

    assert_eq!(
        resolve(".venv/bin/pylsp", false),
        Some(dir.join(".venv/bin/pylsp")),
        "relative to the project root, not to wherever the desktop launched vex from"
    );
    assert_eq!(
        resolve(".venv/bin/other", true),
        Some(bin.join("other.exe")),
        "on Windows the file has a suffix the user did not write, and refusing over that would be \
         pedantry with a broken editor attached"
    );
    assert_eq!(resolve(".venv/bin/missing", false), None);

    // A leading `~` is what people actually write for a tool in their home directory.
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("zls"), "").unwrap();
    assert_eq!(
        vex_lsp::resolve_command_in("~/zls", &dir, Some(&home), &empty, None, false),
        Some(home.join("zls"))
    );

    // And a bare name is still a PATH lookup, which with an empty PATH finds nothing.
    assert_eq!(resolve("pylsp", false), None);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_cmd_shim_resolves_on_windows_and_is_ignored_elsewhere() {
    // The defect this exists for: the installed-check honoured PATHEXT and found
    // `pyright-langserver.cmd`, while the spawn beside it passed the bare name — and Rust's Windows
    // resolver appends only `.exe`, because PATHEXT is a shell concept std does not implement. The
    // two disagreed, so every language server npm installs as a `.cmd` shim passed the check and
    // then failed to start. Resolving to a path is what makes them agree.
    //
    // The platform is a parameter, so the Windows rules are tested wherever this runs.
    let dir = std::env::temp_dir().join(format!("vex-resolve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["pyright-langserver.cmd", "clangd.exe", "pylsp", "gopls.js"] {
        std::fs::write(dir.join(name), "").unwrap();
    }
    let path = std::env::join_paths([&dir]).unwrap();
    let on_windows =
        |program: &str, pathext: &str| vex_lsp::resolve_in(program, &path, Some(pathext), true);

    assert_eq!(
        on_windows("pyright-langserver", ".EXE;.CMD"),
        Some(dir.join("pyright-langserver.cmd")),
        "the npm shim is the whole point"
    );
    assert_eq!(
        on_windows("clangd", ".EXE;.CMD"),
        Some(dir.join("clangd.exe"))
    );
    assert_eq!(
        on_windows("pylsp", ".EXE;.CMD"),
        None,
        "a file with no extension is not a program on Windows"
    );
    assert_eq!(
        on_windows("gopls", ".EXE;.CMD;.JS"),
        None,
        "a real PATHEXT contains .JS, and a stray .js would satisfy the check and then not run"
    );

    assert_eq!(
        vex_lsp::resolve_in("pylsp", &path, None, false),
        Some(dir.join("pylsp")),
        "off Windows the bare name is the program"
    );
    assert_eq!(
        vex_lsp::resolve_in("pyright-langserver", &path, None, false),
        None,
        "and there is no PATHEXT to find the shim with"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The integration test: a real handshake with rust-analyzer.
///
/// Skipped when the server is not installed — a test that fails on a machine without it would be
/// testing the machine, not the code.
#[test]
fn a_real_server_completes_the_handshake_and_reports_diagnostics() {
    // `can_start`, not `is_installed`: on a rustup machine the latter finds a shim that exists whether
    // or not the component does, and speaking LSP to it fails in a way that looks like a vex bug.
    if !vex_lsp::can_start("rust-analyzer") {
        eprintln!("skipping: rust-analyzer is not installed, or is a shim that cannot run");
        return;
    }

    // A minimal crate with a deliberate error, so there is something to diagnose.
    let root = std::env::temp_dir().join(format!("vex-lsp-test-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let source_path = root.join("src/main.rs");
    let source = "fn main() {\n    let x: u32 = \"not a number\";\n}\n";
    std::fs::write(&source_path, source).unwrap();

    let mut client = vex_lsp::LspClient::start(
        &vex_lsp::resolve("rust-analyzer").expect("checked by can_start above"),
        "rust-analyzer",
        &[],
        &root,
    )
    .expect("rust-analyzer starts");

    // Wait for the handshake, then open the file.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut ready = false;
    let mut diagnostics_seen = false;
    let mut messages = Vec::new();

    while std::time::Instant::now() < deadline {
        for event in client.poll() {
            match event {
                vex_lsp::ServerEvent::Ready { server_name, .. } => {
                    assert!(!server_name.is_empty());
                    ready = true;
                    client
                        .did_open(&source_path, "rust", 1, source)
                        .expect("didOpen is accepted");
                }
                vex_lsp::ServerEvent::Diagnostics { path, .. } => {
                    // rust-analyzer reports for several files while it indexes; ours is the one
                    // that matters.
                    if path.ends_with("src/main.rs") {
                        diagnostics_seen = true;
                    }
                }
                vex_lsp::ServerEvent::Message { text } => messages.push(text),
                vex_lsp::ServerEvent::Stopped => break,
                _ => {}
            }
        }

        if diagnostics_seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    client.stop();
    std::fs::remove_dir_all(&root).ok();

    assert!(
        ready,
        "the handshake never completed; server said: {messages:?}"
    );
    // Diagnostics depend on rust-analyzer finishing its initial scan, which can exceed the budget on
    // a loaded machine. The handshake is the assertion; diagnostics are reported either way.
    if !diagnostics_seen {
        eprintln!("note: handshake succeeded but no diagnostics arrived in time");
    }
}

/// A real rename, end to end: ask rust-analyzer to rename a binding and check the edits it sends.
///
/// The value of this test over a mocked one is that the *shape* of a workspace edit is the part most
/// likely to be wrong — `changes` versus `documentChanges`, annotated edits, URI spelling — and only
/// a real server exercises the shape it actually chooses to send.
#[test]
fn a_real_server_renames_a_symbol() {
    // `can_start`, not `is_installed`: on a rustup machine the latter finds a shim that exists whether
    // or not the component does, and speaking LSP to it fails in a way that looks like a vex bug.
    if !vex_lsp::can_start("rust-analyzer") {
        eprintln!("skipping: rust-analyzer is not installed, or is a shim that cannot run");
        return;
    }

    let root = std::env::temp_dir().join(format!("vex-lsp-rename-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let source_path = root.join("src/main.rs");
    // `value` used three times, so a correct rename produces three edits.
    let source = "fn main() {\n    let value = 1;\n    let doubled = value + value;\n    println!(\"{doubled}\");\n}\n";
    std::fs::write(&source_path, source).unwrap();

    let mut client = vex_lsp::LspClient::start(
        &vex_lsp::resolve("rust-analyzer").expect("checked by can_start above"),
        "rust-analyzer",
        &[],
        &root,
    )
    .expect("rust-analyzer starts");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut asked = false;
    let mut renamed: Option<(String, vex_lsp::lsp_types::WorkspaceEdit)> = None;

    // `value`, on line 1 (0-based), character 8.
    fn ask(client: &vex_lsp::LspClient, path: &std::path::Path) {
        client
            .rename(
                path,
                vex_lsp::lsp_types::Position {
                    line: 1,
                    character: 8,
                },
                "total",
            )
            .expect("the rename request is sent");
    }

    while std::time::Instant::now() < deadline && renamed.is_none() {
        for event in client.poll() {
            match event {
                vex_lsp::ServerEvent::Ready { .. } => {
                    client
                        .did_open(&source_path, "rust", 1, source)
                        .expect("didOpen is accepted");
                }
                // Diagnostics mean the crate is analyzed enough to ask. Before that a rename comes
                // back as `ContentModified`, which is the server saying "not yet", not a failure.
                vex_lsp::ServerEvent::Diagnostics { path, .. } if !asked => {
                    if path.ends_with("src/main.rs") {
                        asked = true;
                        ask(&client, &source_path);
                    }
                }
                vex_lsp::ServerEvent::Rename { new_name, edit } => {
                    renamed = Some((new_name, edit));
                }
                // The protocol defines these as retryable, so retrying is what a correct client does.
                vex_lsp::ServerEvent::RequestFailed { reason, retry, .. } if retry => {
                    eprintln!("server: not ready ({reason}) — asking again");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    ask(&client, &source_path);
                }
                vex_lsp::ServerEvent::RequestFailed { what, reason, .. } => {
                    panic!("{what} failed and is not retryable: {reason}")
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    client.stop();
    std::fs::remove_dir_all(&root).ok();

    let Some((new_name, edit)) = renamed else {
        // Analysis of even a one-file crate can exceed the budget on a loaded machine. Reported
        // rather than failed, for the same reason as the diagnostics above.
        eprintln!("note: rust-analyzer did not answer the rename in time");
        return;
    };

    assert_eq!(new_name, "total", "the reply carries what was asked for");

    let edits: Vec<_> = match (&edit.changes, &edit.document_changes) {
        (Some(changes), _) => changes.values().flatten().cloned().collect(),
        (None, Some(vex_lsp::lsp_types::DocumentChanges::Edits(edits))) => edits
            .iter()
            .flat_map(|e| e.edits.iter())
            .map(|e| match e {
                vex_lsp::lsp_types::OneOf::Left(edit) => edit.clone(),
                vex_lsp::lsp_types::OneOf::Right(annotated) => annotated.text_edit.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };

    assert_eq!(
        edits.len(),
        3,
        "`value` appears three times, so three places change: {edits:?}"
    );
    assert!(
        edits.iter().all(|edit| edit.new_text == "total"),
        "every edit inserts the new name: {edits:?}"
    );
}

#[test]
fn a_real_server_sends_types_a_ranking_and_an_import() {
    // Everything the parse used to throw away, against the server that actually sends it. A fixture
    // cannot verify this: the question is not "does vex handle a CompletionItem" but "does what
    // rust-analyzer puts on the wire survive the parse", and only rust-analyzer can answer it.
    if !vex_lsp::can_start("rust-analyzer") {
        eprintln!("skipping: rust-analyzer is not installed, or is a shim that cannot run");
        return;
    }

    let root = std::env::temp_dir().join(format!("vex-lsp-complete-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let source_path = root.join("src/main.rs");
    // `HashMap` is not in scope, so a correct reply offers it *with* the import beside it.
    let source = "fn main() {\n    let m = HashMa\n}\n";
    std::fs::write(&source_path, source).unwrap();

    let mut client = vex_lsp::LspClient::start(
        &vex_lsp::resolve("rust-analyzer").expect("checked by can_start above"),
        "rust-analyzer",
        &[],
        &root,
    )
    .expect("rust-analyzer starts");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut offered: Option<Vec<vex_lsp::CompletionItem>> = None;
    let mut next_ask = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut ready = false;

    // Just after `HashMa`, on line 1 (0-based).
    let ask = |client: &vex_lsp::LspClient, path: &std::path::Path| {
        client
            .completion(
                path,
                vex_lsp::lsp_types::Position {
                    line: 1,
                    character: 18,
                },
            )
            .expect("the completion request is sent");
    };

    // Asked repeatedly, not once. Answering before `std` is indexed is not a failure — it is
    // rust-analyzer returning the generic keyword list, which is what the first reply here actually
    // was, and a test that stopped at it verified nothing about auto-import. A real editor is in
    // the same position: you type, you get what is ready, you type again.
    let is_hash_map = |item: &vex_lsp::CompletionItem| {
        item.label == "HashMap" || item.filter.as_deref() == Some("HashMap")
    };
    let mut resolved: Option<vex_lsp::CompletionItem> = None;

    while std::time::Instant::now() < deadline && resolved.is_none() {
        for event in client.poll() {
            match event {
                vex_lsp::ServerEvent::Ready { .. } => {
                    client
                        .did_open(&source_path, "rust", 1, source)
                        .expect("didOpen is accepted");
                    ready = true;
                }
                vex_lsp::ServerEvent::Completion { items, .. } if !items.is_empty() => {
                    // Its import is *not* in this reply, by design: the client said it can ask
                    // separately, which is the only reason the suggestion is offered at all.
                    if let Some((at, item)) = items.iter().enumerate().find(|(_, i)| is_hash_map(i))
                    {
                        client
                            .resolve_completion(at, &item.label, &item.raw)
                            .expect("the resolve request is sent");
                    }
                    offered = Some(items);
                }
                vex_lsp::ServerEvent::CompletionResolved { item, .. } => resolved = Some(item),
                vex_lsp::ServerEvent::RequestFailed { reason, retry, .. } if retry => {
                    eprintln!("server: not ready ({reason}) — asking again");
                }
                vex_lsp::ServerEvent::RequestFailed { what, reason, .. } => {
                    panic!("{what} failed and is not retryable: {reason}")
                }
                _ => {}
            }
        }
        if ready && std::time::Instant::now() >= next_ask {
            ask(&client, &source_path);
            next_ask = std::time::Instant::now() + std::time::Duration::from_secs(3);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    client.stop();
    std::fs::remove_dir_all(&root).ok();

    let Some(items) = offered else {
        // Analysis of even a one-file crate can exceed the budget on a loaded machine. Reported
        // rather than failed, for the same reason the rename test does it.
        eprintln!("note: rust-analyzer did not answer the completion in time");
        return;
    };

    assert!(
        items.iter().any(|item| !item.kind.is_empty()),
        "a real reply says what each suggestion *is*"
    );
    assert!(
        items.iter().any(|item| item.sort.is_some()),
        "and carries its own ranking, which is better than any the editor can invent"
    );

    let Some(offer) = items.iter().find(|item| is_hash_map(item)) else {
        // Flyimport is a server setting, and a test that failed because some future rust-analyzer
        // defaulted it off would be testing the server rather than vex.
        eprintln!("note: this rust-analyzer offered no out-of-scope HashMap");
        return;
    };

    assert_eq!(
        offer.label, "HashMap",
        "the label is a name: without `labelDetailsSupport` the server folds the whole import path \
         into it, and the column meant for an identifier reads \
         `HashMap(use std::collections::HashMap)`"
    );
    assert!(
        offer
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("std::collections")),
        "and where it would come from is the fact that decides which HashMap you meant: {:?}",
        offer.detail
    );

    let Some(resolved) = resolved else {
        eprintln!("note: rust-analyzer did not resolve the suggestion in time");
        return;
    };
    assert!(
        resolved
            .additional
            .iter()
            .any(|edit| edit.new_text.contains("use std::collections::HashMap")),
        "the headline: completing an out-of-scope name brings its import: {resolved:?}"
    );
}

#[test]
fn a_symbol_hierarchy_flattens_into_an_outline() {
    // The shape rust-analyzer and gopls send: nested, with no URI anywhere, because it is all one
    // file and the server assumes the client remembers which one it asked about.
    let reply = serde_json::json!([
        {
            "name": "Anchor",
            "kind": 23,
            "range": { "start": { "line": 10, "character": 0 },
                       "end": { "line": 40, "character": 1 } },
            "selectionRange": { "start": { "line": 10, "character": 11 },
                                "end": { "line": 10, "character": 17 } },
            "children": [
                {
                    "name": "offset",
                    "kind": 8,
                    "range": { "start": { "line": 11, "character": 4 },
                               "end": { "line": 11, "character": 26 } },
                    "selectionRange": { "start": { "line": 11, "character": 8 },
                                        "end": { "line": 11, "character": 14 } }
                },
                {
                    "name": "new",
                    "kind": 6,
                    "range": { "start": { "line": 20, "character": 4 },
                               "end": { "line": 22, "character": 5 } },
                    "selectionRange": { "start": { "line": 20, "character": 11 },
                                        "end": { "line": 20, "character": 14 } }
                }
            ]
        }
    ]);

    let here = std::path::Path::new("/p/src/anchor.rs");
    let symbols = vex_lsp::symbols_from(&reply, here);

    assert_eq!(symbols.len(), 3, "the tree is flattened, not summarised");
    assert_eq!(
        (symbols[0].name.as_str(), symbols[0].kind),
        ("Anchor", "struct")
    );
    assert_eq!(symbols[0].depth, 0);

    assert_eq!(symbols[1].depth, 1, "and the nesting survives as depth");
    assert_eq!(
        symbols[1].container, "Anchor",
        "the enclosing name is what makes a field read as belonging to something"
    );
    assert_eq!(
        (symbols[2].name.as_str(), symbols[2].kind),
        ("new", "method")
    );

    // The *selection* range, not the whole range: choosing a symbol should land on its name rather
    // than on the first line of a thirty-line body.
    assert_eq!(symbols[2].position.line, 20);
    assert_eq!(symbols[2].position.character, 11);
    assert!(
        symbols.iter().all(|symbol| symbol.path == here),
        "a hierarchy names no file, so every symbol belongs to the one that was asked about"
    );
}

#[test]
fn a_flat_symbol_reply_is_read_too() {
    // What `workspace/symbol` sends, and what an older server sends for a document: no nesting, a
    // `location` carrying the file, and the container as a name rather than a parent.
    let reply = serde_json::json!([
        {
            "name": "canonical",
            "kind": 12,
            "containerName": "vex_project",
            "location": {
                "uri": "file:///p/src/lib.rs",
                "range": { "start": { "line": 7, "character": 7 },
                           "end": { "line": 7, "character": 16 } }
            }
        }
    ]);

    let symbols = vex_lsp::symbols_from(&reply, std::path::Path::new(""));
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].kind, "fn");
    assert_eq!(symbols[0].container, "vex_project");
    assert_eq!(symbols[0].position.line, 7);
    assert_eq!(
        symbols[0].path,
        std::path::PathBuf::from("/p/src/lib.rs"),
        "a project-wide result has to say which file it is in, or it cannot be opened"
    );
}

#[test]
fn a_symbol_reply_that_makes_no_sense_yields_nothing_rather_than_panicking() {
    // Servers are not proofs. A reply with no array, or entries with no name, must come back empty
    // rather than taking the editor with it.
    let path = std::path::Path::new("/p/x.rs");
    assert!(vex_lsp::symbols_from(&serde_json::json!(null), path).is_empty());
    assert!(vex_lsp::symbols_from(&serde_json::json!({ "items": [] }), path).is_empty());
    assert!(vex_lsp::symbols_from(&serde_json::json!([{ "kind": 12 }]), path).is_empty());

    // A name with nothing else is still a symbol; it just has no position to offer.
    let bare = vex_lsp::symbols_from(&serde_json::json!([{ "name": "x" }]), path);
    assert_eq!(bare.len(), 1);
    assert_eq!(bare[0].position.line, 0);
    assert_eq!(bare[0].kind, "", "and no kind is better than a wrong one");
}

// ---- URIs on both platforms --------------------------------------------------------------------

use vex_lsp::position::{decode_uri, encode_uri};

#[test]
fn a_windows_path_becomes_a_uri_a_language_server_accepts() {
    // The bug this exists for: vex sent `file://` followed by a percent-encoded backslash, so every
    // server read the path as a *host* and rejected the request. Every LSP feature was broken on
    // Windows, and only a Windows CI run found it — hence the platform being a parameter here.
    assert_eq!(
        encode_uri(r"C:\Users\someone\project\main.rs", true),
        "file:///C:/Users/someone/project/main.rs"
    );

    // `canonicalize` returns an extended-length path. The prefix is not part of the file's name.
    assert_eq!(
        encode_uri(r"\\?\C:\Users\someone\main.rs", true),
        "file:///C:/Users/someone/main.rs"
    );

    // A network share keeps its double slash, which is the authority this time.
    assert_eq!(
        encode_uri(r"\\?\UNC\server\share\main.rs", true),
        "file:////server/share/main.rs"
    );
}

#[test]
fn a_unix_path_is_untouched_apart_from_escaping() {
    assert_eq!(
        encode_uri("/home/someone/project/main.rs", false),
        "file:///home/someone/project/main.rs"
    );
    assert_eq!(
        encode_uri("/home/someone/my project/main.rs", false),
        "file:///home/someone/my%20project/main.rs",
        "a space is escaped; a slash is not"
    );
}

#[test]
fn uris_round_trip_on_both_platforms() {
    for (path, windows) in [
        (r"C:\Users\someone\main.rs", true),
        (r"C:\Users\some one\a+b.rs", true),
        ("/home/someone/main.rs", false),
        ("/home/someone/my project/main.rs", false),
        ("/tmp/vex-lsp-test-3284/src/main.rs", false),
    ] {
        let uri = encode_uri(path, windows);
        assert_eq!(
            decode_uri(&uri, windows).as_deref(),
            Some(path),
            "{path} did not survive the round trip through {uri}"
        );
    }
}

#[test]
fn a_uri_has_an_empty_authority_so_the_path_is_never_read_as_a_host() {
    // `file://C:/x` names a *host* called `C:`. This is the exact shape rust-analyzer refused.
    for (path, windows) in [(r"C:\x\y.rs", true), ("/x/y.rs", false)] {
        let uri = encode_uri(path, windows);
        assert!(
            uri.starts_with("file:///"),
            "{uri} must have three slashes, or the path is parsed as a host"
        );
    }
}

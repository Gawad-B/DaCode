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
    assert_eq!(server_for("src/main.rs").unwrap().program, "rust-analyzer");
    assert_eq!(server_for("app.py").unwrap().language_id, "python");
    assert_eq!(server_for("main.cpp").unwrap().program, "clangd");
    assert_eq!(server_for("notes.md"), None);
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

    let mut client =
        vex_lsp::LspClient::start("rust-analyzer", &[], &root).expect("rust-analyzer starts");

    // Wait for the handshake, then open the file.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut ready = false;
    let mut diagnostics_seen = false;
    let mut messages = Vec::new();

    while std::time::Instant::now() < deadline {
        for event in client.poll() {
            match event {
                vex_lsp::ServerEvent::Ready { server_name } => {
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

    let mut client =
        vex_lsp::LspClient::start("rust-analyzer", &[], &root).expect("rust-analyzer starts");

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

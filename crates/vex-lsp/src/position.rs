//! Positions, across the one boundary where getting it wrong is invisible until it hurts.
//!
//! LSP measures columns in **UTF-16 code units**. The editor measures them in `char`s. A single
//! astral-plane character — one emoji in a comment — makes those disagree, and every diagnostic,
//! hover and definition after it points at the wrong column. Nothing crashes; the editor is simply
//! subtly wrong in a file that contains an emoji.
//!
//! This is what the typed coordinate spaces from `vex-text` were for: the conversion is a named
//! function with tests, not an integer passed hopefully across a boundary.

use lsp_types::{Position, Range};
use std::ops::Range as StdRange;
use vex_text::{CharOffset, Point, Text, Utf16Column};

/// Editor position → LSP position.
pub fn to_lsp(text: &Text, offset: CharOffset) -> Position {
    let point = text.char_to_point(offset);
    let (line, column) = text.point_to_lsp(point);
    Position {
        line: line as u32,
        character: column.get() as u32,
    }
}

/// LSP position → editor offset.
///
/// Out-of-range positions are clamped rather than rejected: a server working from a slightly older
/// document version will send positions past the end, and clamping keeps the editor usable while
/// the version check decides whether to trust the result at all.
pub fn from_lsp(text: &Text, position: Position) -> CharOffset {
    let point = text.lsp_to_point(
        position.line as usize,
        Utf16Column(position.character as usize),
    );
    text.point_to_char(point)
}

pub fn range_to_lsp(text: &Text, range: StdRange<CharOffset>) -> Range {
    Range {
        start: to_lsp(text, range.start),
        end: to_lsp(text, range.end),
    }
}

pub fn range_from_lsp(text: &Text, range: Range) -> StdRange<CharOffset> {
    let start = from_lsp(text, range.start);
    let end = from_lsp(text, range.end);
    // Servers occasionally send inverted ranges; ordering them here beats every consumer guarding.
    if start <= end { start..end } else { end..start }
}

/// The `Point` an LSP position names, for display.
pub fn point_from_lsp(text: &Text, position: Position) -> Point {
    text.lsp_to_point(
        position.line as usize,
        Utf16Column(position.character as usize),
    )
}

/// A `file://` URI for a path.
///
/// Hand-built rather than pulled from a URL crate: the editor needs exactly this one conversion, and
/// percent-encoding the handful of characters that matter is smaller than the dependency.
pub fn path_to_uri(path: &std::path::Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    encode_uri(&absolute.to_string_lossy(), cfg!(windows))
}

/// The URI form of a path string, for a given platform.
///
/// Split out with `windows` as a parameter so both platforms' rules are testable from either. The
/// alternative is what happened here: a Windows build sent `file://%5C%5C%3F%5CC%3A%5CUsers%5C...` —
/// an extended-length Windows path percent-encoded whole — and rust-analyzer rejected every request
/// with "invalid international domain name", because `file://` followed by anything but a slash is
/// parsed as a *host*. Every language-server feature was broken on Windows, and no test running on
/// Linux could have noticed.
///
/// Three things have to happen on Windows and none of them on Unix: strip the extended-length prefix
/// that `canonicalize` adds, turn separators into forward slashes, and put a slash before the drive
/// letter so that the authority component is empty.
pub fn encode_uri(path: &str, windows: bool) -> String {
    let mut text = path.to_string();

    if windows {
        // What `canonicalize` returns on Windows: `\\?\C:\x`, or `\\?\UNC\server\share` for a network
        // path. Neither prefix belongs in a URI.
        if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
            text = format!("\\\\{rest}");
        } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
            text = rest.to_string();
        }
        text = text.replace('\\', "/");
    }

    // `file://`, an empty authority, then an absolute path. A drive letter is not absolute by itself,
    // so it needs the slash.
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/');
    }

    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            // Left literal so a drive letter reads as `C:` rather than `C%3A`. Every language server
            // accepts it, and several print URIs back to the user.
            b':' => out.push(':'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The path a `file://` URI names.
pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    decode_uri(uri, cfg!(windows)).map(std::path::PathBuf::from)
}

/// The path form of a URI, for a given platform. The inverse of [`encode_uri`].
pub fn decode_uri(uri: &str, windows: bool) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut index = 0;

    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[index + 1..index + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                bytes.push(byte);
                index += 3;
                continue;
            }
        }
        bytes.push(raw[index]);
        index += 1;
    }

    let mut text = String::from_utf8(bytes).ok()?;

    if windows {
        // A leading slash before a drive letter was ours to add and is ours to remove. A UNC path
        // (`//server/share`) keeps both of its slashes.
        let looks_like_drive = text.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
            && text.as_bytes().get(2) == Some(&b':');
        if !text.starts_with("//") && text.starts_with('/') && looks_like_drive {
            text.remove(0);
        }
        text = text.replace('/', "\\");
    }

    Some(text)
}

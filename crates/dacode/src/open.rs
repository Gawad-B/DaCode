//! Reading a file into a buffer, with the refusals said out loud.
//!
//! Opening used to be a bare `read_to_string` whose only failure path was an `eprintln!` — to a
//! console a release build on Windows does not have. So dragging in a 900MB log stalled the window
//! with no explanation, an executable opened as a rope full of replacement characters, and a
//! Latin-1 file simply did nothing at all. Three different outcomes, all of them looking like the
//! editor had frozen or ignored you.
//!
//! Every refusal here is a sentence the caller can put on screen. That is the whole point: an
//! editor may decline to open something, but it may not decline silently.
//!
//! **The honest limitation.** This is still synchronous, on the UI thread. What has changed is that
//! it is now *bounded* — the size cap is what makes the worst case finite — and that is a different
//! claim from "asynchronous". Moving the read to a worker with a `Version` tag, the way syntax, git
//! and the file walk already work, is the real fix and is a larger change than this one.

use std::path::Path;

/// The largest file DaCode will read into a buffer.
///
/// Generous on purpose: handling large files well is meant to be a strength, not something to
/// apologise for, so this is a guard against pathological input rather than a working limit. A
/// 128MiB source file is not a file anyone is editing by hand.
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// How much of the file is sniffed for a NUL before deciding it is not text.
const SNIFF_BYTES: usize = 8 * 1024;

/// What came back.
#[derive(Debug)]
pub enum Opened {
    Text(String),
    /// Why not, in words meant for the person who asked.
    Refused(String),
}

/// Reads a file, or says why it will not.
pub fn read(path: &Path) -> Opened {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());

    match std::fs::metadata(path) {
        Ok(data) if data.len() > MAX_BYTES => {
            return Opened::Refused(format!(
                "{name} is {} and dacode opens files up to {}",
                describe_size(data.len()),
                describe_size(MAX_BYTES)
            ));
        }
        Ok(data) if data.is_dir() => {
            return Opened::Refused(format!("{name} is a folder — ctrl-k o opens one"));
        }
        // A missing file is not an error here: the caller turns it into a scratch buffer, which is
        // how `dacode newfile.rs` is meant to work.
        Err(err) => return Opened::Refused(format!("could not open {name}: {err}")),
        Ok(_) => {}
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => return Opened::Refused(format!("could not read {name}: {err}")),
    };

    // A NUL byte early on means this is not text. Without the check an executable opens as a rope
    // of replacement characters, which is unreadable, enormous, and saveable over the original.
    if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return Opened::Refused(format!("{name} looks like a binary file, not text"));
    }

    match String::from_utf8(bytes) {
        Ok(text) => Opened::Text(text),
        // Decoding it lossily would let a save write the replacement characters back over the
        // original file — quietly destroying whatever the bytes were. Refusing is the honest
        // answer until DaCode can actually decode other encodings.
        Err(_) => Opened::Refused(format!(
            "{name} is not valid UTF-8, and dacode cannot open other encodings yet"
        )),
    }
}

fn describe_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.0}MB", bytes as f64 / MIB)
    } else {
        format!("{}KB", bytes / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dacode-open-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn refusal(opened: Opened) -> String {
        match opened {
            Opened::Refused(why) => why,
            Opened::Text(_) => panic!("expected a refusal"),
        }
    }

    #[test]
    fn ordinary_text_opens() {
        let dir = scratch("text");
        let file = dir.join("hello.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        match read(&file) {
            Opened::Text(text) => assert_eq!(text, "fn main() {}\n"),
            Opened::Refused(why) => panic!("refused ordinary text: {why}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_binary_file_is_refused_by_name() {
        // The failure this prevents: an executable opened as a rope of replacement characters,
        // which is unreadable and — worse — saveable back over the original.
        let dir = scratch("binary");
        let file = dir.join("thing.bin");
        std::fs::write(&file, [0x7f, b'E', b'L', b'F', 0x00, 0x01, 0x02]).unwrap();

        let why = refusal(read(&file));
        assert!(
            why.contains("thing.bin"),
            "the message must name the file: {why}"
        );
        assert!(why.contains("binary"), "and say what is wrong: {why}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_in_another_encoding_is_refused_rather_than_mangled() {
        // Lossy decoding would let a later save write replacement characters over the original.
        let dir = scratch("latin1");
        let file = dir.join("notes.txt");
        // `café` in Latin-1: the 0xE9 is not valid UTF-8 on its own.
        std::fs::write(&file, [b'c', b'a', b'f', 0xE9, b'\n']).unwrap();

        let why = refusal(read(&file));
        assert!(why.contains("UTF-8"), "{why}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_says_so() {
        let why = refusal(read(Path::new("/definitely/not/here.rs")));
        assert!(why.contains("here.rs"), "{why}");
    }

    #[test]
    fn sizes_read_the_way_people_write_them() {
        assert_eq!(describe_size(128 * 1024 * 1024), "128MB");
        assert_eq!(describe_size(4 * 1024), "4KB");
    }
}

//! File discovery, fuzzy ranking and project search.

use std::path::{Path, PathBuf};
use vex_project::{
    FileIndex, FileWalk, ProjectFile, ProjectSearch, SearchQuery, find_root, search_file,
};

/// A throwaway project tree. Named per test so parallel tests cannot collide.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("vex-project-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

/// Waits for the walk, with a deadline so a hang fails loudly instead of blocking the suite.
fn wait_for_walk(walk: &FileWalk) -> Vec<ProjectFile> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(result) = walk.poll() {
            return result.files;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the file walk never finished"
        );
        std::thread::yield_now();
    }
}

#[test]
fn the_walk_finds_files_and_makes_paths_relative() {
    let fixture = Fixture::new("walk");
    fixture.write("src/main.rs", "fn main() {}");
    fixture.write("src/lib.rs", "pub fn go() {}");
    fixture.write("README.md", "# hi");

    let files = wait_for_walk(&FileWalk::start(fixture.path()));
    let mut names: Vec<&str> = files.iter().map(|f| f.relative.as_str()).collect();
    names.sort_unstable();

    assert_eq!(names, vec!["README.md", "src/lib.rs", "src/main.rs"]);
}

#[test]
fn gitignored_and_hidden_files_are_excluded() {
    // A file list containing `target/` is worse than no list: it buries every real result.
    let fixture = Fixture::new("ignore");
    fixture.write(".gitignore", "target/\n*.log\n");
    fixture.write("src/main.rs", "fn main() {}");
    fixture.write("target/debug/huge.rs", "// build output");
    fixture.write("noisy.log", "log line");
    fixture.write(".secret", "hidden");

    let files = wait_for_walk(&FileWalk::start(fixture.path()));
    let names: Vec<&str> = files.iter().map(|f| f.relative.as_str()).collect();

    assert!(names.contains(&"src/main.rs"));
    assert!(
        !names.iter().any(|name| name.starts_with("target/")),
        "ignored directory leaked in: {names:?}"
    );
    assert!(
        !names.contains(&"noisy.log"),
        "ignored glob leaked in: {names:?}"
    );
    assert!(
        !names.contains(&".secret"),
        "hidden file leaked in: {names:?}"
    );
}

#[test]
fn a_file_knows_its_name_and_directory() {
    let file = ProjectFile {
        relative: "crates/vex-text/src/anchor.rs".into(),
        absolute: PathBuf::from("/tmp/x/crates/vex-text/src/anchor.rs"),
    };
    assert_eq!(file.name(), "anchor.rs");
    assert_eq!(file.directory(), "crates/vex-text/src");

    let bare = ProjectFile {
        relative: "README.md".into(),
        absolute: PathBuf::from("/tmp/x/README.md"),
    };
    assert_eq!(bare.name(), "README.md");
    assert_eq!(bare.directory(), "");
}

fn index_of(paths: &[&str]) -> FileIndex {
    let mut index = FileIndex::new("/project");
    index.set_files(
        paths
            .iter()
            .map(|relative| ProjectFile {
                relative: (*relative).to_string(),
                absolute: PathBuf::from("/project").join(relative),
            })
            .collect(),
        false,
    );
    index
}

#[test]
fn fuzzy_matching_ranks_the_obvious_answer_first() {
    let mut index = index_of(&[
        "crates/vex-text/src/anchor.rs",
        "crates/vex-input/src/action.rs",
        "crates/vex-render/src/frame.rs",
        "docs/architecture.md",
    ]);

    let matches = index.search("anchor", 10);
    assert!(!matches.is_empty());
    assert_eq!(matches[0].file.name(), "anchor.rs");

    // Subsequence matching: initials of path segments should find the file.
    let matches = index.search("vtsa", 10);
    assert_eq!(
        matches.first().map(|m| m.file.relative.as_str()),
        Some("crates/vex-text/src/anchor.rs"),
        "got {:?}",
        matches.iter().map(|m| &m.file.relative).collect::<Vec<_>>()
    );
}

#[test]
fn match_positions_are_returned_for_highlighting() {
    // The design highlights exactly the characters the matcher reports, so they must come back.
    let mut index = index_of(&["src/anchor.rs"]);
    let matches = index.search("anch", 10);

    let first = &matches[0];
    assert!(!first.positions.is_empty(), "no positions reported");
    assert!(
        first.positions.windows(2).all(|w| w[0] < w[1]),
        "positions must be sorted and unique: {:?}",
        first.positions
    );

    let chars: Vec<char> = first.file.relative.chars().collect();
    let matched: String = first
        .positions
        .iter()
        .map(|index| chars[*index as usize])
        .collect();
    assert_eq!(matched.to_lowercase(), "anch");
}

#[test]
fn shorter_paths_win_ties() {
    // `src/lib.rs` is nearly always what was meant over `vendor/dep/src/lib.rs`.
    let mut index = index_of(&["vendor/dep/src/lib.rs", "src/lib.rs"]);
    let matches = index.search("lib.rs", 10);
    assert_eq!(matches[0].file.relative, "src/lib.rs");
}

#[test]
fn an_empty_query_lists_files_rather_than_nothing() {
    // Opening the palette should show something before anything is typed.
    let mut index = index_of(&["a.rs", "b.rs", "c.rs"]);
    let matches = index.search("", 2);
    assert_eq!(matches.len(), 2, "limit is respected");
    assert_eq!(matches[0].file.relative, "a.rs");
}

#[test]
fn a_query_matching_nothing_returns_nothing() {
    let mut index = index_of(&["a.rs", "b.rs"]);
    assert!(index.search("zzzzqqq", 10).is_empty());
}

#[test]
fn the_limit_bounds_the_result_list() {
    let paths: Vec<String> = (0..500).map(|i| format!("src/file{i}.rs")).collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let mut index = index_of(&refs);

    assert_eq!(index.search("file", 20).len(), 20);
}

#[test]
fn literal_search_finds_a_plain_string() {
    let fixture = Fixture::new("literal");
    let path = fixture.write(
        "src/main.rs",
        "fn main() {\n    let total = 1 + 2;\n    println!(\"{total}\");\n}\n",
    );

    let mut hits = Vec::new();
    search_file(&path, &SearchQuery::literal("total"), &mut hits).unwrap();

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].line_number, 2);
    assert!(hits[0].line.contains("let total"));
    // The reported range must point at the match, not the whole line.
    assert_eq!(&hits[0].line[hits[0].range.clone()], "total");
}

#[test]
fn literal_search_does_not_treat_the_pattern_as_a_regex() {
    // Searching for `a.c` must not match `abc`, or "literal" means nothing.
    let fixture = Fixture::new("escape");
    let path = fixture.write("f.txt", "abc\na.c\n");

    let mut hits = Vec::new();
    search_file(&path, &SearchQuery::literal("a.c"), &mut hits).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, "a.c");
}

#[test]
fn regex_search_works_and_a_bad_pattern_is_an_error_not_a_panic() {
    let fixture = Fixture::new("regex");
    let path = fixture.write("f.rs", "fn alpha() {}\nfn beta() {}\n");

    let mut hits = Vec::new();
    search_file(&path, &SearchQuery::regex(r"fn \w+\(\)"), &mut hits).unwrap();
    assert_eq!(hits.len(), 2);

    let mut hits = Vec::new();
    assert!(
        search_file(&path, &SearchQuery::regex("(unclosed"), &mut hits).is_err(),
        "an invalid pattern must be reported, not panic"
    );
}

#[test]
fn smart_case_is_insensitive_until_you_type_a_capital() {
    let fixture = Fixture::new("case");
    let path = fixture.write("f.txt", "Total\ntotal\n");

    let mut lower = Vec::new();
    search_file(&path, &SearchQuery::literal("total"), &mut lower).unwrap();
    assert_eq!(lower.len(), 2, "a lowercase query matches both");

    let mut upper = Vec::new();
    search_file(&path, &SearchQuery::literal("Total"), &mut upper).unwrap();
    assert_eq!(upper.len(), 1, "an uppercase letter means case matters");
    assert_eq!(upper[0].line, "Total");
}

#[test]
fn whole_word_search_does_not_match_inside_words() {
    let fixture = Fixture::new("word");
    let path = fixture.write("f.txt", "total\nsubtotal\n");

    let query = SearchQuery {
        pattern: "total".into(),
        literal: true,
        whole_word: true,
        ..Default::default()
    };
    let mut hits = Vec::new();
    search_file(&path, &query, &mut hits).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, "total");
}

#[test]
fn the_hit_limit_is_respected() {
    let fixture = Fixture::new("limit");
    let contents: String = (0..200).map(|_| "match\n").collect();
    let path = fixture.write("f.txt", &contents);

    let query = SearchQuery {
        pattern: "match".into(),
        literal: true,
        limit: 10,
        ..Default::default()
    };
    let mut hits = Vec::new();
    search_file(&path, &query, &mut hits).unwrap();
    assert_eq!(hits.len(), 10);
}

#[test]
fn project_search_streams_hits_and_honors_ignore_files() {
    let fixture = Fixture::new("project-search");
    fixture.write(".gitignore", "ignored/\n");
    fixture.write("src/a.rs", "let needle = 1;\n");
    fixture.write("src/b.rs", "// no match here\n");
    fixture.write("ignored/c.rs", "let needle = 2;\n");

    let search = ProjectSearch::start(fixture.path(), SearchQuery::literal("needle"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut hits = Vec::new();
    loop {
        hits.extend(search.poll());
        if search.is_finished() {
            hits.extend(search.poll());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the search never finished"
        );
        std::thread::yield_now();
    }

    assert_eq!(hits.len(), 1, "found {hits:?}");
    assert!(hits[0].path.ends_with("src/a.rs"));
}

#[test]
fn a_cancelled_search_stops() {
    let fixture = Fixture::new("cancel");
    for i in 0..50 {
        fixture.write(&format!("src/f{i}.rs"), "needle\n");
    }

    let search = ProjectSearch::start(fixture.path(), SearchQuery::literal("needle"));
    search.cancel();

    // Cancelling must not hang or panic; whatever arrived before the flag was seen is fine.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !search.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "cancel did not stop the search"
        );
        std::thread::yield_now();
    }
}

#[test]
fn the_project_root_is_found_by_its_marker() {
    let fixture = Fixture::new("root");
    fixture.write("Cargo.toml", "[package]\n");
    let nested = fixture.write("crates/inner/src/lib.rs", "");

    // Compared canonically: /tmp is a symlink on some systems.
    let expected = vex_project::canonical(fixture.path());
    assert_eq!(find_root(&nested), expected);
    assert_eq!(find_root(fixture.path()), expected);
}

#[test]
fn a_repository_root_beats_a_member_manifest() {
    // The bug this guards: in a Cargo workspace the first `Cargo.toml` walking up is a *member*
    // manifest, so preferring it would index one crate and call that the project.
    let fixture = Fixture::new("workspace");
    std::fs::create_dir_all(fixture.path().join(".git")).unwrap();
    fixture.write("Cargo.toml", "[workspace]\n");
    fixture.write("crates/inner/Cargo.toml", "[package]\n");
    let nested = fixture.write("crates/inner/src/lib.rs", "");

    assert_eq!(
        find_root(&nested),
        vex_project::canonical(fixture.path()),
        "should find the repository root, not the member crate"
    );
}

#[test]
fn a_directory_with_no_marker_is_its_own_root() {
    // Opening a stray file must not index a whole home directory.
    let fixture = Fixture::new("no-marker");
    let file = fixture.write("notes.txt", "");
    let root = find_root(&file);

    // Both sides canonicalized: on Windows `temp_dir()` can be an 8.3 short name while a resolved path
    // is the long one, and comparing the two forms fails for a reason that has nothing to do with
    // finding roots. This test failed on Windows CI on exactly that.
    let temp = vex_project::canonical(&std::env::temp_dir());
    assert!(
        root.starts_with(&temp),
        "should not walk above the temp dir: {root:?} is not under {temp:?}"
    );
}

#[test]
fn windows_extended_length_prefixes_are_not_part_of_a_path() {
    use vex_project::strip_extended_prefix;

    // What `canonicalize` returns on Windows. Correct for the filesystem API, wrong everywhere else —
    // it reaches the statusline, path comparisons and URIs.
    assert_eq!(
        strip_extended_prefix(r"\\?\C:\Users\someone"),
        r"C:\Users\someone"
    );
    // A network path keeps one slash, because `\\server\share` is its real form.
    assert_eq!(
        strip_extended_prefix(r"\\?\UNC\server\share"),
        r"\server\share"
    );
    // Anything else is left exactly as it is, including every Unix path.
    assert_eq!(strip_extended_prefix("/home/someone"), "/home/someone");
    assert_eq!(
        strip_extended_prefix(r"C:\already\plain"),
        r"C:\already\plain"
    );
}

// ---- what a text field is allowed to name ------------------------------------------------------

/// The root every containment test is written against. Never touched on disk: `within` is lexical,
/// which is exactly the property being pinned down.
fn root() -> std::path::PathBuf {
    if cfg!(windows) {
        std::path::PathBuf::from(r"C:\work\project")
    } else {
        std::path::PathBuf::from("/work/project")
    }
}

#[test]
fn an_ordinary_relative_name_resolves_under_the_root() {
    let resolved = vex_project::within(&root(), "src/new/thing.rs").expect("inside the project");
    assert!(resolved.starts_with(root()));
    assert!(resolved.ends_with("thing.rs"));
}

#[test]
fn a_parent_step_that_climbs_above_the_root_is_refused_even_when_nothing_exists() {
    // The defect. The old check canonicalized the parent and compared with `starts_with`:
    // `canonicalize` fails with NotFound the moment a component is missing — which is the normal
    // case for a file being created — and `vex_project::canonical` then returns its input
    // unchanged, `..` chain intact. `starts_with` compares components, and components normalise `.`
    // but never `..`, so the chain matched the root on its first two and the file was written
    // wherever the `..`s landed.
    //
    // Nothing on this machine could see it: Win32 collapses `..` before the syscall, so the
    // platform vex is developed on caught the attack by accident and Linux did not.
    for escape in [
        "../evil.txt",
        "../../evil.txt",
        "x/../../evil.txt",
        "x/../../../../evil.txt",
        "a/b/../../../c.txt",
    ] {
        assert_eq!(
            vex_project::within(&root(), escape),
            None,
            "`{escape}` escaped the project root"
        );
    }
}

#[test]
fn an_absolute_name_is_refused_rather_than_replacing_the_root() {
    // `Path::join` with an absolute argument discards the left side entirely. The old check caught
    // these, but only by accident: it canonicalized the parent, and `/etc` and `C:\Windows` happen
    // to exist. A drive or share that did not would have gone through on the same `NotFound`
    // fallback as the `..` case. Refused for a reason now rather than by luck.
    for absolute in ["/etc/passwd", r"C:\Windows\system32\x", r"\server\share\x"] {
        assert_eq!(
            vex_project::within(&root(), absolute),
            None,
            "`{absolute}` replaced the project root"
        );
    }
}

#[test]
fn a_parent_step_that_stays_inside_the_root_is_allowed_and_folded_away() {
    // `..` is not itself the problem; climbing out is. And the folded path is what comes back, so
    // what the caller writes to is what was checked — the old version returned the unfolded join,
    // so `a.rs` and `./a.rs` compared as different files in rename.
    let resolved = vex_project::within(&root(), "src/old/../new/thing.rs").expect("inside");
    assert_eq!(resolved, root().join("src").join("new").join("thing.rs"));

    let dotted = vex_project::within(&root(), "./a.rs").expect("inside");
    assert_eq!(dotted, root().join("a.rs"));
}

#[test]
fn a_name_that_resolves_to_the_root_itself_is_refused() {
    // The root is a directory. Nothing here may create, rename or delete it, and returning it would
    // hand a directory to `fs::write`.
    for naming_the_root in [".", "src/..", "./."] {
        assert_eq!(vex_project::within(&root(), naming_the_root), None);
    }
    assert_eq!(vex_project::within(&root(), "   "), None);
}

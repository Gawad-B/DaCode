//! What File ▸ Copy link to line copies, and what the Run button offers to run.
//!
//! **Share** copies a permanent link to the cursor's line on the repository's host — pinned to the
//! commit that is checked out, so the link still points at the same line after the file changes —
//! or `path:line` when the project has no remote anyone else could open. A local editor has no
//! account and no server to share through; a link to the line is what people paste into a review,
//! an issue or a chat. It was a Share button in the title bar; it is a File entry now, beside the
//! other things done to the file, and the command is still `share`.
//!
//! The repository is read from its own files, `.git/config` and `HEAD`, not by running `git`: these
//! are a few small files, and a process spawn is exactly the kind of unbounded wait that must not
//! happen on the thread drawing the window.
//!
//! **Run** runs the open file, or the project when the file belongs to its toolchain, and the event
//! loop types the command into the terminal. It went on the clipboard before there was a terminal
//! to type it into. It once knew only projects, so a Python script on its own — the commonest thing
//! anyone presses Run on — was told there was nothing to run.

use std::path::Path;

/// A link to `line` (1-based) of `file`, or `relative:line` when there is nowhere to link to.
pub fn link(root: &Path, file: &Path, line: usize) -> String {
    let relative = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    let fallback = format!("{relative}:{line}");

    let git = root.join(".git");
    let (Some(config), Some(head)) = (
        std::fs::read_to_string(git.join("config")).ok(),
        std::fs::read_to_string(git.join("HEAD")).ok(),
    ) else {
        return fallback;
    };
    let Some(remote) = origin_url(&config).and_then(|url| web_base(&url)) else {
        return fallback;
    };
    let Some(commit) = resolve_head(&git, &head) else {
        return fallback;
    };
    format_link(&remote, &commit, &relative, line)
}

/// The `url` of `[remote "origin"]`, or of the first remote when there is no origin.
fn origin_url(config: &str) -> Option<String> {
    let mut section = String::new();
    let mut first = None;
    for raw in config.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "url" || !section.starts_with("[remote ") {
            continue;
        }
        let value = value.trim().to_string();
        if section == "[remote \"origin\"]" {
            return Some(value);
        }
        first.get_or_insert(value);
    }
    first
}

/// A remote's address as the web page for the repository: host and path, no `.git`, no user.
///
/// SSH (`git@host:owner/repo.git`, `ssh://git@host/owner/repo`) and HTTPS forms both appear in the
/// wild, often in the same team, and they name the same page.
fn web_base(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (host, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .or_else(|| url.strip_prefix("git://"))
    {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        let (host, path) = rest.split_once('/')?;
        // A port belongs to the transport, not to the page.
        (host.split(':').next()?.to_string(), path.to_string())
    } else {
        // scp-like: user@host:owner/repo
        let rest = url.rsplit_once('@').map_or(url, |(_, after)| after);
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    };
    (!host.is_empty() && !path.is_empty()).then_some((host, path))
}

/// The commit `HEAD` names, following one symbolic reference through loose and packed refs.
fn resolve_head(git: &Path, head: &str) -> Option<String> {
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:").map(str::trim) else {
        return is_commit(head).then(|| head.to_string());
    };
    if let Ok(loose) = std::fs::read_to_string(git.join(reference)) {
        let loose = loose.trim();
        if is_commit(loose) {
            return Some(loose.to_string());
        }
    }
    let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (commit, name) = line.split_once(' ')?;
        (name.trim() == reference && is_commit(commit)).then(|| commit.to_string())
    })
}

fn is_commit(text: &str) -> bool {
    (40..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The link, in the shape each host's pages use for "this line at this commit".
fn format_link(
    (host, path): &(String, String),
    commit: &str,
    relative: &str,
    line: usize,
) -> String {
    let base = format!("https://{host}/{path}");
    if host.contains("gitlab") {
        format!("{base}/-/blob/{commit}/{relative}#L{line}")
    } else if host.contains("bitbucket") {
        format!("{base}/src/{commit}/{relative}#lines-{line}")
    } else {
        // GitHub's shape, which Codeberg, Gitea and Forgejo share.
        format!("{base}/blob/{commit}/{relative}#L{line}")
    }
}

/// The command the Run button types: the project's when `file` is part of that project's
/// toolchain, otherwise one that runs `file` itself, otherwise the project's anyway.
///
/// `windows` picks PowerShell's quoting and chaining, which is the shell the terminal starts there,
/// and `py` over `python3`, which is what python.org's installer puts on `PATH`. It is a parameter
/// so those rules have tests that run here.
pub fn run_command_for(root: &Path, file: Option<&Path>, windows: bool) -> Option<String> {
    let project = run_command(root);
    let Some(file) = file else {
        return project;
    };
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if let Some(project) = &project {
        let tool = project.split_whitespace().next().unwrap_or_default();
        let belongs = match extension.as_str() {
            "rs" => tool == "cargo",
            "dart" => tool == "flutter" || tool == "dart",
            "go" => tool == "go",
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "vue" | "svelte" => {
                ["npm", "pnpm", "yarn", "bun"].contains(&tool)
            }
            // A Django project is run as a server whatever module is open; a script beside a
            // `main.py` is still that script.
            "py" => project.contains("manage.py"),
            "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" => tool == "make",
            _ => true,
        };
        if belongs {
            return Some(project.clone());
        }
    }
    file_command(file, &extension, windows).or(project)
}

/// A command that runs one file by its kind, or `None` for a kind nobody runs directly.
fn file_command(file: &Path, extension: &str, windows: bool) -> Option<String> {
    let path = quote(&file.to_string_lossy(), windows);
    let built = |compile: String| {
        let binary = file.with_extension(if windows { "exe" } else { "" });
        let binary = quote(&binary.to_string_lossy(), windows);
        let compile = compile.replace("{out}", &binary);
        // `&&` is not PowerShell 5's, which is the one every Windows machine has.
        if windows {
            format!("{compile}; if ($?) {{ & {binary} }}")
        } else {
            format!("{compile} && {binary}")
        }
    };
    let interpreted = |program: &str| Some(format!("{program} {path}"));
    match extension {
        "py" | "pyw" => interpreted(if windows { "py" } else { "python3" }),
        "js" | "mjs" | "cjs" => interpreted("node"),
        "ts" | "mts" => interpreted("deno run"),
        "rb" => interpreted("ruby"),
        "lua" => interpreted("lua"),
        "pl" => interpreted("perl"),
        "php" => interpreted("php"),
        "r" => interpreted("Rscript"),
        "jl" => interpreted("julia"),
        "dart" => interpreted("dart run"),
        "go" => interpreted("go run"),
        "kts" => interpreted("kotlinc -script"),
        "java" => interpreted("java"),
        "sh" | "bash" => interpreted(if windows { "bash" } else { "sh" }),
        "zsh" => interpreted("zsh"),
        "ps1" => interpreted("powershell -File"),
        "bat" | "cmd" if windows => Some(format!("& {path}")),
        "rs" => Some(built(format!("rustc {path} -o {{out}}"))),
        "c" => Some(built(format!("gcc {path} -o {{out}}"))),
        "cc" | "cpp" | "cxx" => Some(built(format!("g++ {path} -o {{out}}"))),
        _ => None,
    }
}

/// `text` as one argument to the shell the terminal runs: single quotes for a POSIX shell, double
/// for PowerShell, and left bare when nothing in it needs either.
fn quote(text: &str, windows: bool) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/\\._-:+".contains(c));
    if plain {
        text.to_string()
    } else if windows {
        format!(
            "\"{}\"",
            text.replace('`', "``")
                .replace('"', "`\"")
                .replace('$', "`$")
        )
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The command a project of this kind is usually run with, judged from the files at its root.
///
/// The first match wins, in the order of how specific the marker is: a Flutter app also has a
/// `pubspec.yaml` that plain Dart has, so the file is read to tell them apart.
pub fn run_command(root: &Path) -> Option<String> {
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        return Some("cargo run".into());
    }
    if has("pubspec.yaml") {
        let flutter = std::fs::read_to_string(root.join("pubspec.yaml"))
            .is_ok_and(|text| text.contains("flutter:"));
        return Some(if flutter { "flutter run" } else { "dart run" }.into());
    }
    if has("package.json") {
        let scripts = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
        let manager = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        for script in ["dev", "start"] {
            if scripts.contains(&format!("\"{script}\"")) {
                return Some(format!("{manager} run {script}"));
            }
        }
        return Some(format!("{manager} start"));
    }
    if has("go.mod") {
        return Some("go run .".into());
    }
    if has("manage.py") {
        return Some("python manage.py runserver".into());
    }
    if has("main.py") {
        return Some("python main.py".into());
    }
    if has("Makefile") {
        return Some("make".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dacode-share-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        dir
    }

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_github_remote_gets_a_permanent_link_to_the_line() {
        let root = scratch("github");
        std::fs::write(
            root.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:someone/project.git\n",
        )
        .unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(root.join(".git/refs/heads/main"), format!("{COMMIT}\n")).unwrap();

        assert_eq!(
            link(&root, &root.join("src/main.rs"), 27),
            format!("https://github.com/someone/project/blob/{COMMIT}/src/main.rs#L27")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_packed_branch_and_a_gitlab_remote_still_link() {
        // A freshly cloned repository has no loose ref for its branch: it is in packed-refs.
        let root = scratch("packed");
        std::fs::write(
            root.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://gitlab.com/group/sub/project.git\n",
        )
        .unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/dev\n").unwrap();
        std::fs::write(
            root.join(".git/packed-refs"),
            format!("# pack-refs with: peeled\n{COMMIT} refs/heads/dev\n"),
        )
        .unwrap();

        assert_eq!(
            link(&root, &root.join("lib/a.dart"), 3),
            format!("https://gitlab.com/group/sub/project/-/blob/{COMMIT}/lib/a.dart#L3")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn with_no_remote_the_link_is_the_path_and_line() {
        let root = scratch("local");
        std::fs::write(root.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        std::fs::write(root.join(".git/HEAD"), format!("{COMMIT}\n")).unwrap();
        assert_eq!(link(&root, &root.join("notes.md"), 1), "notes.md:1");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_remote_spelling_names_the_same_page() {
        let page = Some(("github.com".to_string(), "a/b".to_string()));
        for url in [
            "git@github.com:a/b.git",
            "https://github.com/a/b",
            "https://user@github.com/a/b.git",
            "ssh://git@github.com:22/a/b.git",
            "https://github.com/a/b/",
        ] {
            assert_eq!(web_base(url), page, "{url}");
        }
    }

    #[test]
    fn a_project_is_run_the_way_its_kind_usually_is() {
        let root = scratch("run");
        assert_eq!(
            run_command(&root),
            None,
            "nothing to go on, nothing offered"
        );
        std::fs::write(root.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(run_command(&root).as_deref(), Some("pnpm run dev"));
        std::fs::write(root.join("pubspec.yaml"), "name: app\nflutter:\n").unwrap();
        assert_eq!(run_command(&root).as_deref(), Some("flutter run"));
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(run_command(&root).as_deref(), Some("cargo run"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_lone_script_is_run_by_its_own_interpreter() {
        // The case the button used to answer with "no run command found": a Python file in a
        // folder with nothing else to say what it is.
        let root = scratch("script");
        let file = root.join("hello.py");
        let shown = file.to_string_lossy().into_owned();
        assert_eq!(
            run_command_for(&root, Some(&file), false),
            Some(format!("python3 {shown}"))
        );
        assert_eq!(
            run_command_for(&root, Some(&file), true),
            Some(format!("py {shown}")),
            "python.org's installer puts the launcher on PATH, not python3"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_of_the_projects_toolchain_runs_the_project() {
        let root = scratch("toolchain");
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        let rust = root.join("src/main.rs");
        assert_eq!(
            run_command_for(&root, Some(&rust), false).as_deref(),
            Some("cargo run")
        );
        // A helper script in a Rust project is not the Rust project.
        let script = root.join("tools/gen.py");
        assert!(
            run_command_for(&root, Some(&script), false)
                .unwrap()
                .starts_with("python3 ")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_compiled_file_is_built_then_run_in_each_shells_own_syntax() {
        let file = Path::new("/w/a.c");
        assert_eq!(
            run_command_for(Path::new("/nowhere"), Some(file), false).as_deref(),
            Some("gcc /w/a.c -o /w/a && /w/a")
        );
        let file = Path::new(r"C:\w\a.cpp");
        assert_eq!(
            run_command_for(Path::new("/nowhere"), Some(file), true).as_deref(),
            Some(r"g++ C:\w\a.cpp -o C:\w\a.exe; if ($?) { & C:\w\a.exe }"),
            "PowerShell 5 has no &&"
        );
    }

    #[test]
    fn a_path_with_spaces_or_quotes_reaches_the_shell_as_one_argument() {
        assert_eq!(quote("/home/me/my file.py", false), "'/home/me/my file.py'");
        assert_eq!(quote("/it's.py", false), r"'/it'\''s.py'");
        assert_eq!(
            quote(r"C:\My Files\a$b.py", true),
            r#""C:\My Files\a`$b.py""#
        );
    }

    #[test]
    fn a_file_nobody_runs_falls_back_to_the_project_or_nothing() {
        let root = scratch("unknown");
        let notes = root.join("notes.txt");
        assert_eq!(run_command_for(&root, Some(&notes), false), None);
        std::fs::write(root.join("Makefile"), "all:\n").unwrap();
        assert_eq!(
            run_command_for(&root, Some(&notes), false).as_deref(),
            Some("make")
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

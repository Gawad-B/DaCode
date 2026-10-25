//! What the Share button copies, and what the Run button offers to run.
//!
//! **Share** copies a permanent link to the cursor's line on the repository's host — pinned to the
//! commit that is checked out, so the link still points at the same line after the file changes —
//! or `path:line` when the project has no remote anyone else could open. A local editor has no
//! account and no server to share through; a link to the line is what people paste into a review,
//! an issue or a chat, and it is what the button in an editor's title bar is reached for.
//!
//! The repository is read from its own files, `.git/config` and `HEAD`, not by running `git`: these
//! are a few small files, and a process spawn is exactly the kind of unbounded wait that must not
//! happen on the thread drawing the window.
//!
//! **Run** has no terminal to run anything in yet. Rather than a button that does nothing, it finds
//! the command this kind of project is run with and puts it on the clipboard, saying so.

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
}

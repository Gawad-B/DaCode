//! Formatters and language servers that DaCode downloads for itself, so a fresh machine needs
//! nothing installed first: `dacode tools install pyright ruff`.
//!
//! # Why downloaded, and why not bundled
//!
//! Bundling every server makes an installer of a gigabyte or more — pyright and the TypeScript
//! server need Node, and Node alone is thirty megabytes per platform — and stale the day after
//! release. Asking a package manager (`install.rs`) needs that package manager, which a fresh
//! Windows machine, or a Linux one without pip or npm, does not have. So each tool here is a pinned
//! release, fetched only when someone asks for it, for the platform it is running on.
//!
//! # What is trusted
//!
//! **Every download is checked against a SHA-256 written in this file**, not one fetched beside it.
//! The updater checks a release against sums from the same release, which catches corruption; a
//! pinned hash also catches a replaced asset, because whoever replaces the file cannot replace the
//! number compiled into the editor. The cost is that versions are pinned, and moving to a newer one
//! is an edit here. npm packages are pinned by version, and npm checks each tarball against the
//! registry's own integrity hash — that trusts the registry, and the table says which tools do.
//!
//! # Where they go
//!
//! `<state>/tools/` — `~/.local/share/dacode/tools` on Linux, `%LOCALAPPDATA%\dacode\tools` on
//! Windows, machine-local because a binary for this machine means nothing on another. Each tool is
//! unpacked into a folder of its own under `pkgs/`, and a launcher for each program it provides goes
//! in `bin/`: a symbolic link, a two-line `sh` script for an npm tool (which then needs no `node` on
//! `PATH`), or a `.cmd` on Windows. `main` appends `bin/` to the *end* of `PATH` once at startup, so
//! a server or formatter the user installed themselves is always found first.
//!
//! # The limits, said
//!
//! Linux on x86-64 and ARM64 and Windows on x86-64 only — there is no macOS packaging of DaCode to
//! serve. clangd publishes no ARM64 Linux build, and the musl builds are used where a project offers
//! them so a distribution's C library does not matter; rust-analyzer, clangd and Node are glibc
//! builds and will not run on Alpine. Nothing here updates by itself: `dacode tools install` again
//! after DaCode moves a pin is how a tool is updated.

use std::path::{Path, PathBuf};

/// The platforms a tool can be downloaded for.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Platform {
    LinuxX64,
    LinuxArm64,
    WindowsX64,
}

impl Platform {
    /// This machine, or `None` for one nothing here is built for.
    pub fn current() -> Option<Platform> {
        Platform::of(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// From the names `std::env::consts` uses, so the rules have tests on any machine.
    pub fn of(os: &str, arch: &str) -> Option<Platform> {
        match (os, arch) {
            ("linux", "x86_64") => Some(Platform::LinuxX64),
            ("linux", "aarch64") => Some(Platform::LinuxArm64),
            ("windows", "x86_64") => Some(Platform::WindowsX64),
            _ => None,
        }
    }

    pub fn is_windows(self) -> bool {
        self == Platform::WindowsX64
    }

    fn describe(self) -> &'static str {
        match self {
            Platform::LinuxX64 => "64-bit Linux",
            Platform::LinuxArm64 => "ARM64 Linux",
            Platform::WindowsX64 => "64-bit Windows",
        }
    }
}

/// How a download is packed.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Archive {
    /// One gzipped file: the program itself.
    Gz,
    TarGz,
    Zip,
}

/// One file to download for one platform.
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    pub platform: Platform,
    pub url: &'static str,
    pub sha256: &'static str,
    pub archive: Archive,
    /// Where the program is inside the unpacked folder.
    pub program: &'static str,
}

/// Where a tool comes from.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// A release file per platform.
    Release(&'static [Asset]),
    /// npm packages, installed with the portable Node DaCode downloads for them: the packages
    /// (`name@version`), and for each program provided, the package whose `bin` names it.
    Npm {
        packages: &'static [&'static str],
        programs: &'static [(&'static str, &'static str)],
    },
}

/// A formatter, a language server, or the Node the npm ones run on.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub version: &'static str,
    /// What it is, for `dacode tools list`.
    pub what: &'static str,
    /// The programs it puts on `PATH`. For a `Release` tool, one.
    pub provides: &'static [&'static str],
    pub source: Source,
}

const NODE_VERSION: &str = "24.22.0";

/// The Node the npm tools run on. Not offered on its own: it is installed with the first npm tool
/// that needs it, and it gets no launcher, so a terminal's `node` stays the user's.
pub const NODE: Tool = Tool {
    name: "node",
    version: NODE_VERSION,
    what: "the JavaScript runtime the npm tools run on",
    provides: &[],
    source: Source::Release(&[
        Asset {
            platform: Platform::LinuxX64,
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-linux-x64.tar.gz",
            sha256: "28ea0a438de9a13bd7891fd758eee6c7819ee8af5442dfd8d45bd65e56d4228c",
            archive: Archive::TarGz,
            program: "node-v24.22.0-linux-x64/bin/node",
        },
        Asset {
            platform: Platform::LinuxArm64,
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-linux-arm64.tar.gz",
            sha256: "b7e7991d5407815adfa74a2a9cd4db683552c8ce32ff4a295c22e972849c3f5d",
            archive: Archive::TarGz,
            program: "node-v24.22.0-linux-arm64/bin/node",
        },
        Asset {
            platform: Platform::WindowsX64,
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-win-x64.zip",
            sha256: "f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3",
            archive: Archive::Zip,
            program: "node-v24.22.0-win-x64/node.exe",
        },
    ]),
};

/// Every tool DaCode can download, by the name `dacode tools install` takes.
pub const TOOLS: [Tool; 8] = [
    Tool {
        name: "rust-analyzer",
        version: "2026-10-12",
        what: "Rust language server",
        provides: &["rust-analyzer"],
        source: Source::Release(&[
            Asset {
                platform: Platform::LinuxX64,
                url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-10-12/rust-analyzer-x86_64-unknown-linux-gnu.gz",
                sha256: "040cab4e2abc490fadd95c3f6cbc60b941a78761dbc3acf0ec66fd7a97f31db3",
                archive: Archive::Gz,
                program: "rust-analyzer",
            },
            Asset {
                platform: Platform::LinuxArm64,
                url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-10-12/rust-analyzer-aarch64-unknown-linux-gnu.gz",
                sha256: "d7219d5d8f5537bb227b521157aebddfe7e4a90f4a85a74f31e53722f5691025",
                archive: Archive::Gz,
                program: "rust-analyzer",
            },
            Asset {
                platform: Platform::WindowsX64,
                url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-10-12/rust-analyzer-x86_64-pc-windows-msvc.zip",
                sha256: "77f7a907f0e6f179281bac38e2d08435c974db9a5b99de52c5c963aff52883ec",
                archive: Archive::Zip,
                program: "rust-analyzer.exe",
            },
        ]),
    },
    Tool {
        name: "ruff",
        version: "0.17.0",
        what: "Python formatter and linter",
        provides: &["ruff"],
        source: Source::Release(&[
            Asset {
                platform: Platform::LinuxX64,
                url: "https://github.com/astral-sh/ruff/releases/download/0.17.0/ruff-x86_64-unknown-linux-musl.tar.gz",
                sha256: "0df9421fd0df9aa7a62dab4d1086e085b7c7888b6343fc5a9c5f630771d61aae",
                archive: Archive::TarGz,
                program: "ruff-x86_64-unknown-linux-musl/ruff",
            },
            Asset {
                platform: Platform::LinuxArm64,
                url: "https://github.com/astral-sh/ruff/releases/download/0.17.0/ruff-aarch64-unknown-linux-musl.tar.gz",
                sha256: "cac779097e34bf1ca40175b4bacf8fca7a92fce5030bfc3edf1fd357428a33b0",
                archive: Archive::TarGz,
                program: "ruff-aarch64-unknown-linux-musl/ruff",
            },
            Asset {
                platform: Platform::WindowsX64,
                url: "https://github.com/astral-sh/ruff/releases/download/0.17.0/ruff-x86_64-pc-windows-msvc.zip",
                sha256: "bc831eaa01912323099d7bc185981bf6bb1960931350bd3ad6a82f7a59a46c3b",
                archive: Archive::Zip,
                program: "ruff.exe",
            },
        ]),
    },
    Tool {
        name: "clangd",
        version: "23.1.0",
        what: "C and C++ language server",
        provides: &["clangd"],
        source: Source::Release(&[
            Asset {
                platform: Platform::LinuxX64,
                url: "https://github.com/clangd/clangd/releases/download/23.1.0/clangd-linux-23.1.0.zip",
                sha256: "e53b1a96196095faedb7642cf64964f7fb9ad4a0c1f00dd2c172a3d9dcbafdfd",
                archive: Archive::Zip,
                program: "clangd_23.1.0/bin/clangd",
            },
            Asset {
                platform: Platform::WindowsX64,
                url: "https://github.com/clangd/clangd/releases/download/23.1.0/clangd-windows-23.1.0.zip",
                sha256: "23412a240756a162e7b98a282f36aa2a23a88db5ce16a0cbc4fef7253768c810",
                archive: Archive::Zip,
                program: "clangd_23.1.0/bin/clangd.exe",
            },
        ]),
    },
    Tool {
        name: "lua-language-server",
        version: "3.19.2",
        what: "Lua language server",
        provides: &["lua-language-server"],
        source: Source::Release(&[
            Asset {
                platform: Platform::LinuxX64,
                url: "https://github.com/LuaLS/lua-language-server/releases/download/3.19.2/lua-language-server-3.19.2-linux-x64.tar.gz",
                sha256: "93ad326b10f6c00911dd8c450fa098332dbd8f9f45d1052f5636a2d54479b947",
                archive: Archive::TarGz,
                program: "bin/lua-language-server",
            },
            Asset {
                platform: Platform::LinuxArm64,
                url: "https://github.com/LuaLS/lua-language-server/releases/download/3.19.2/lua-language-server-3.19.2-linux-arm64.tar.gz",
                sha256: "17b8e401a874440adeb649e5024c364f2bb9d15156c74a40e56d1ae9a1b2e4f2",
                archive: Archive::TarGz,
                program: "bin/lua-language-server",
            },
            Asset {
                platform: Platform::WindowsX64,
                url: "https://github.com/LuaLS/lua-language-server/releases/download/3.19.2/lua-language-server-3.19.2-win32-x64.zip",
                sha256: "4a3dc851b3b31e7b75a23f645d1ba4a2137579a55f5fa4edbb17a3e2a6254044",
                archive: Archive::Zip,
                program: "bin/lua-language-server.exe",
            },
        ]),
    },
    Tool {
        name: "stylua",
        version: "2.6.0",
        what: "Lua formatter",
        provides: &["stylua"],
        source: Source::Release(&[
            Asset {
                platform: Platform::LinuxX64,
                url: "https://github.com/JohnnyMorganz/StyLua/releases/download/v2.6.0/stylua-linux-x86_64-musl.zip",
                sha256: "48ee7db28346f96f4bd545e190ab080a88a7b5a664e97649cf5d5c60426f402d",
                archive: Archive::Zip,
                program: "stylua",
            },
            Asset {
                platform: Platform::LinuxArm64,
                url: "https://github.com/JohnnyMorganz/StyLua/releases/download/v2.6.0/stylua-linux-aarch64-musl.zip",
                sha256: "278495597e3827417f3af7b9ad38011ad4a5ec725503553160ab69b7301059bb",
                archive: Archive::Zip,
                program: "stylua",
            },
            Asset {
                platform: Platform::WindowsX64,
                url: "https://github.com/JohnnyMorganz/StyLua/releases/download/v2.6.0/stylua-windows-x86_64.zip",
                sha256: "fd13b4014e3b779c244bbc03f2c282e357f15bc526de6a6fa5ecbc447538313c",
                archive: Archive::Zip,
                program: "stylua.exe",
            },
        ]),
    },
    Tool {
        name: "pyright",
        version: "1.1.414",
        what: "Python language server (npm, on DaCode's Node)",
        provides: &["pyright-langserver"],
        source: Source::Npm {
            packages: &["pyright@1.1.414"],
            programs: &[("pyright-langserver", "pyright")],
        },
    },
    Tool {
        name: "typescript-language-server",
        version: "6.0.2",
        what: "JavaScript and TypeScript language server (npm, on DaCode's Node)",
        provides: &["typescript-language-server"],
        source: Source::Npm {
            // TypeScript 7 is the native compiler and ships no `tsserver`, which this server
            // drives; 6 is the newest that does.
            packages: &["typescript-language-server@6.0.2", "typescript@6.0.3"],
            programs: &[("typescript-language-server", "typescript-language-server")],
        },
    },
    Tool {
        name: "prettier",
        version: "3.9.10",
        what: "JavaScript, TypeScript, JSON, CSS and Markdown formatter (npm, on DaCode's Node)",
        provides: &["prettier"],
        source: Source::Npm {
            packages: &["prettier@3.9.10"],
            programs: &[("prettier", "prettier")],
        },
    },
];

/// A tool by its name.
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// Whether `tool` can be downloaded for `platform` — clangd has no ARM64 Linux build.
pub fn available(tool: &Tool, platform: Platform) -> bool {
    match tool.source {
        Source::Release(assets) => assets.iter().any(|asset| asset.platform == platform),
        Source::Npm { .. } => available(&NODE, platform),
    }
}

/// `<state>/tools`.
pub fn root() -> PathBuf {
    crate::paths::state_dir().join("tools")
}

/// Where the launchers are: the directory appended to `PATH`.
pub fn bin_dir(root: &Path) -> PathBuf {
    root.join("bin")
}

/// The folder a tool's version is unpacked into.
fn package_dir(root: &Path, tool: &Tool) -> PathBuf {
    root.join("pkgs")
        .join(format!("{}-{}", tool.name, tool.version))
}

/// Whether `tool` at its pinned version is installed: its folder is there and so is each launcher.
pub fn installed(root: &Path, tool: &Tool, windows: bool) -> bool {
    package_dir(root, tool).is_dir()
        && tool
            .provides
            .iter()
            .all(|program| std::fs::symlink_metadata(launcher(root, program, windows)).is_ok())
}

fn launcher(root: &Path, program: &str, windows: bool) -> PathBuf {
    let name = if windows {
        format!("{program}.cmd")
    } else {
        program.to_string()
    };
    bin_dir(root).join(name)
}

/// Installs `tool` for `platform` under `root`, fetching through `fetch` and reporting each step
/// to `say`. Already installed is success. Every failure is a sentence for the person who asked.
pub fn install(
    root: &Path,
    tool: &Tool,
    platform: Platform,
    fetch: &dyn crate::update::Fetch,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let windows = platform.is_windows();
    if installed(root, tool, windows) {
        say(&format!(
            "{} {} is already installed",
            tool.name, tool.version
        ));
        return Ok(());
    }
    if !available(tool, platform) {
        return Err(format!(
            "{} has no download for {} — install it yourself, and DaCode will find it on PATH",
            tool.name,
            platform.describe()
        ));
    }
    std::fs::create_dir_all(bin_dir(root))
        .map_err(|error| format!("could not create {}: {error}", bin_dir(root).display()))?;
    match tool.source {
        Source::Release(assets) => {
            let asset = assets
                .iter()
                .find(|asset| asset.platform == platform)
                .expect("checked by `available`");
            let folder = unpack_release(root, tool, asset, fetch, say)?;
            let program = tool.provides.first().copied().unwrap_or(tool.name);
            write_launcher(
                root,
                program,
                &Launch::Program(folder.join(asset.program)),
                windows,
            )?;
        }
        Source::Npm { packages, programs } => {
            install_node(root, platform, fetch, say)?;
            let node = node_program(root, platform);
            let folder = package_dir(root, tool);
            npm_install(platform, &node, &folder, packages, say)?;
            for (program, package) in programs {
                let entry = npm_entry(&folder, package, program)?;
                write_launcher(
                    root,
                    program,
                    &Launch::Node {
                        node: node.clone(),
                        entry,
                    },
                    windows,
                )?;
            }
        }
    }
    say(&format!("installed {} {}", tool.name, tool.version));
    Ok(())
}

fn node_asset(platform: Platform) -> Option<&'static Asset> {
    let Source::Release(assets) = NODE.source else {
        return None;
    };
    assets.iter().find(|asset| asset.platform == platform)
}

fn install_node(
    root: &Path,
    platform: Platform,
    fetch: &dyn crate::update::Fetch,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    if node_program(root, platform).is_file() {
        return Ok(());
    }
    let asset = node_asset(platform).ok_or("there is no Node download for this platform")?;
    say(&format!(
        "the npm tools need Node {NODE_VERSION}, which DaCode keeps for them"
    ));
    unpack_release(root, &NODE, asset, fetch, say).map(|_| ())
}

fn node_program(root: &Path, platform: Platform) -> PathBuf {
    let inside = node_asset(platform).map_or("node", |asset| asset.program);
    package_dir(root, &NODE).join(inside)
}

/// Downloads `asset`, checks it against its pinned hash, and unpacks it into the tool's folder.
/// Unpacked beside the final folder and renamed into place, so an interrupted install leaves no
/// folder that [`installed`] would take for a finished one.
fn unpack_release(
    root: &Path,
    tool: &Tool,
    asset: &Asset,
    fetch: &dyn crate::update::Fetch,
    say: &mut dyn FnMut(&str),
) -> Result<PathBuf, String> {
    let downloads = root.join("downloads");
    std::fs::create_dir_all(&downloads)
        .map_err(|error| format!("could not create {}: {error}", downloads.display()))?;
    let file_name = asset.url.rsplit('/').next().unwrap_or(tool.name);
    let part = downloads.join(format!("{file_name}.part"));
    say(&format!("downloading {} {}", tool.name, tool.version));
    fetch.file(asset.url, &part)?;
    let sum = crate::update::sha256_file(&part)
        .map_err(|error| format!("could not read the download: {error}"))?;
    if !sum.eq_ignore_ascii_case(asset.sha256) {
        std::fs::remove_file(&part).ok();
        return Err(format!(
            "the {} download did not match the checksum DaCode expects, so it was not installed",
            tool.name
        ));
    }

    let folder = package_dir(root, tool);
    let staging = folder.with_extension("unpacking");
    std::fs::remove_dir_all(&staging).ok();
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("could not create {}: {error}", staging.display()))?;
    let unpacked = unpack(&part, asset.archive, &staging, asset.program);
    std::fs::remove_file(&part).ok();
    if let Err(why) = unpacked {
        std::fs::remove_dir_all(&staging).ok();
        return Err(format!("could not unpack {}: {why}", tool.name));
    }
    let program = staging.join(asset.program);
    if !program.is_file() {
        std::fs::remove_dir_all(&staging).ok();
        return Err(format!(
            "the {} download has no {} in it",
            tool.name, asset.program
        ));
    }
    make_executable(&program)?;
    std::fs::remove_dir_all(&folder).ok();
    std::fs::rename(&staging, &folder)
        .map_err(|error| format!("could not move {} into place: {error}", tool.name))?;
    Ok(folder)
}

/// Unpacks `archive` into `into`. A `Gz` is one file, written as `program`. Every path inside an
/// archive is checked to stay inside `into`: an entry named `../../.bashrc` is refused, not obeyed.
pub fn unpack(archive: &Path, kind: Archive, into: &Path, program: &str) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    match kind {
        Archive::Gz => {
            let mut decoder = flate2::read::GzDecoder::new(file);
            let mut out =
                std::fs::File::create(into.join(program)).map_err(|error| error.to_string())?;
            std::io::copy(&mut decoder, &mut out).map_err(|error| error.to_string())?;
            Ok(())
        }
        Archive::TarGz => {
            let mut tarball = tar::Archive::new(flate2::read::GzDecoder::new(file));
            for entry in tarball.entries().map_err(|error| error.to_string())? {
                let mut entry = entry.map_err(|error| error.to_string())?;
                // `unpack_in` declines an entry whose path would climb out of `into`, and says so
                // by returning false rather than by an error.
                if !entry.unpack_in(into).map_err(|error| error.to_string())? {
                    return Err("the archive tried to write outside its folder".into());
                }
            }
            Ok(())
        }
        Archive::Zip => {
            let mut zip = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
                let Some(relative) = entry.enclosed_name() else {
                    return Err("the archive tried to write outside its folder".into());
                };
                let target = into.join(relative);
                if entry.is_dir() {
                    std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
                    continue;
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let mut out = std::fs::File::create(&target).map_err(|error| error.to_string())?;
                std::io::copy(&mut entry, &mut out).map_err(|error| error.to_string())?;
                if let Some(mode) = entry.unix_mode() {
                    set_mode(&target, mode);
                }
            }
            Ok(())
        }
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777)).ok();
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions()
        .mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o755))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// What a launcher starts.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    Program(PathBuf),
    /// A script run by DaCode's own Node, named in full so no `node` is needed on `PATH`.
    Node {
        node: PathBuf,
        entry: PathBuf,
    },
}

/// The launcher's text, or `None` where it is a symbolic link instead (a program on Unix).
///
/// On Windows a `.cmd`: `Command::new` on one goes through `cmd.exe` with std's own escaping, the
/// route an npm-installed server's shim has always taken (`dacode_lsp::resolve`). `%*` passes the
/// arguments on as given.
pub fn launcher_text(launch: &Launch, windows: bool) -> Option<String> {
    let quoted = |path: &Path| path.display().to_string().replace('\'', "'\\''");
    match (launch, windows) {
        (Launch::Program(_), false) => None,
        (Launch::Program(program), true) => Some(format!("@\"{}\" %*\r\n", program.display())),
        (Launch::Node { node, entry }, false) => Some(format!(
            "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
            quoted(node),
            quoted(entry)
        )),
        (Launch::Node { node, entry }, true) => Some(format!(
            "@\"{}\" \"{}\" %*\r\n",
            node.display(),
            entry.display()
        )),
    }
}

fn write_launcher(
    root: &Path,
    program: &str,
    launch: &Launch,
    windows: bool,
) -> Result<(), String> {
    let path = launcher(root, program, windows);
    std::fs::remove_file(&path).ok();
    let written = match (launcher_text(launch, windows), launch) {
        (Some(text), _) => std::fs::write(&path, text).map_err(|error| error.to_string()),
        (None, Launch::Program(target)) => symlink(target, &path).map_err(|e| e.to_string()),
        (None, Launch::Node { .. }) => Err("a Node launcher is always a script".to_string()),
    };
    written
        .and_then(|()| make_executable_launcher(&path))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// A script launcher has to be executable; a symbolic link takes its target's mode.
fn make_executable_launcher(path: &Path) -> Result<(), String> {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|data| data.file_type().is_symlink());
    if is_link {
        Ok(())
    } else {
        make_executable(path)
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "symbolic links are only made on Unix",
    ))
}

/// Where npm itself is inside an unpacked Node: beside `node.exe` on Windows, under `lib/` beside
/// `bin/` elsewhere.
fn npm_cli(node: &Path, windows: bool) -> PathBuf {
    let bin = node.parent().unwrap_or(node);
    if windows {
        bin.join("node_modules/npm/bin/npm-cli.js")
    } else {
        bin.parent()
            .unwrap_or(bin)
            .join("lib/node_modules/npm/bin/npm-cli.js")
    }
}

/// Runs npm — the copy that comes inside DaCode's Node — to install `packages` into `folder`.
fn npm_install(
    platform: Platform,
    node: &Path,
    folder: &Path,
    packages: &[&str],
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let staging = folder.with_extension("unpacking");
    std::fs::remove_dir_all(&staging).ok();
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("could not create {}: {error}", staging.display()))?;
    say(&format!("installing {} with npm", packages.join(" ")));
    // npm runs packages' scripts through `node` found on PATH, so this Node's folder goes first —
    // for this child alone; the editor's own PATH is not touched.
    let mut path = std::ffi::OsString::from(node.parent().unwrap_or(node));
    if let Some(existing) = std::env::var_os("PATH") {
        path.push(if platform.is_windows() { ";" } else { ":" });
        path.push(existing);
    }
    let output = crate::process::command_at(node)
        .arg(npm_cli(node, platform.is_windows()))
        .args([
            "install",
            "--no-fund",
            "--no-audit",
            "--no-save",
            "--prefix",
        ])
        .arg(&staging)
        .args(packages)
        .env("PATH", path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("could not run npm: {error}"))?;
    if !output.status.success() {
        std::fs::remove_dir_all(&staging).ok();
        let errors = String::from_utf8_lossy(&output.stderr);
        let last = errors
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("npm failed");
        return Err(format!(
            "npm could not install {}: {last}",
            packages.join(" ")
        ));
    }
    std::fs::remove_dir_all(folder).ok();
    std::fs::rename(&staging, folder)
        .map_err(|error| format!("could not move the npm packages into place: {error}"))
}

/// The script an npm package's `bin` names for `program`.
pub fn npm_entry(folder: &Path, package: &str, program: &str) -> Result<PathBuf, String> {
    let package_dir = folder.join("node_modules").join(package);
    let manifest = std::fs::read_to_string(package_dir.join("package.json"))
        .map_err(|error| format!("{package} was not installed: {error}"))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).map_err(|error| format!("{package}: {error}"))?;
    let relative = match &manifest["bin"] {
        serde_json::Value::String(only) => Some(only.as_str()),
        serde_json::Value::Object(bins) => bins.get(program).and_then(|value| value.as_str()),
        _ => None,
    }
    .ok_or_else(|| format!("{package} does not provide {program}"))?;
    Ok(package_dir.join(relative))
}

/// The `PATH` with DaCode's launchers appended — last, so whatever the user installed is found
/// first. `None` when they are already on it.
pub fn path_with_tools(path: Option<&std::ffi::OsStr>, root: &Path) -> Option<std::ffi::OsString> {
    let bin = bin_dir(root);
    let mut parts: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    if parts.contains(&bin) {
        return None;
    }
    parts.push(bin);
    std::env::join_paths(parts).ok()
}

/// `dacode tools …`: `list`, or `install NAME…`. Returns the process's exit code.
pub fn run_cli(args: &[String]) -> i32 {
    let root = root();
    let Some(platform) = Platform::current() else {
        eprintln!("dacode tools: nothing is downloadable for this platform");
        return 1;
    };
    match args.first().map(String::as_str) {
        Some("list") | None => {
            for tool in &TOOLS {
                let state = if installed(&root, tool, platform.is_windows()) {
                    "installed"
                } else if available(tool, platform) {
                    "available"
                } else {
                    "not for this machine"
                };
                println!(
                    "{:28} {:12} {:22} {}",
                    tool.name, tool.version, state, tool.what
                );
            }
            println!("\ninto {}", root.display());
            0
        }
        Some("install") if args.len() > 1 => {
            let mut failed = false;
            for name in &args[1..] {
                let Some(tool) = find(name) else {
                    eprintln!(
                        "dacode tools: no tool called {name} — `dacode tools list` names them"
                    );
                    failed = true;
                    continue;
                };
                let mut say = |line: &str| println!("{line}");
                if let Err(why) = install(&root, tool, platform, &crate::update::Curl, &mut say) {
                    eprintln!("{why}");
                    failed = true;
                }
            }
            i32::from(failed)
        }
        _ => {
            eprintln!("usage: dacode tools list\n       dacode tools install NAME…");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dacode-tools-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Serves fixed bytes for fixed URLs, so no test reaches the network.
    struct Fake(HashMap<String, Vec<u8>>);

    impl crate::update::Fetch for Fake {
        fn text(&self, url: &str) -> Result<String, String> {
            Err(format!("no text at {url}"))
        }
        fn file(&self, url: &str, to: &Path) -> Result<(), String> {
            let bytes = self.0.get(url).ok_or_else(|| format!("no {url}"))?;
            std::fs::write(to, bytes).map_err(|error| error.to_string())
        }
    }

    fn sha256(bytes: &[u8]) -> &'static str {
        let mut hasher = crate::update::Sha256::default();
        hasher.update(bytes);
        Box::leak(crate::update::hex(&hasher.finish()).into_boxed_str())
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            for (name, body) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(body).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn tool_from(asset: Asset) -> Tool {
        Tool {
            name: "fake",
            version: "1.0",
            what: "a test tool",
            provides: &["fake"],
            source: Source::Release(Box::leak(vec![asset].into_boxed_slice())),
        }
    }

    #[test]
    fn the_platform_is_read_from_the_names_std_uses() {
        assert_eq!(Platform::of("linux", "x86_64"), Some(Platform::LinuxX64));
        assert_eq!(Platform::of("linux", "aarch64"), Some(Platform::LinuxArm64));
        assert_eq!(
            Platform::of("windows", "x86_64"),
            Some(Platform::WindowsX64)
        );
        assert_eq!(
            Platform::of("macos", "aarch64"),
            None,
            "no macOS packaging to serve"
        );
    }

    #[test]
    fn every_tool_is_offered_for_every_platform_but_clangd_on_arm() {
        // A table row with a platform missing is a button that fails after the click; this keeps
        // the one known gap the only one.
        for tool in &TOOLS {
            for platform in [
                Platform::LinuxX64,
                Platform::LinuxArm64,
                Platform::WindowsX64,
            ] {
                let expected = !(tool.name == "clangd" && platform == Platform::LinuxArm64);
                assert_eq!(
                    available(tool, platform),
                    expected,
                    "{} on {platform:?}",
                    tool.name
                );
            }
            if let Source::Release(assets) = tool.source {
                for asset in assets {
                    assert_eq!(asset.sha256.len(), 64, "{}", asset.url);
                    assert!(asset.url.starts_with("https://"), "{}", asset.url);
                    assert!(
                        (asset.platform == Platform::WindowsX64) == asset.program.ends_with(".exe"),
                        "{}",
                        asset.url
                    );
                }
            }
        }
    }

    #[test]
    fn a_release_is_downloaded_checked_unpacked_and_given_a_launcher() {
        // For the machine running the test, as `install` is only ever asked for: a Linux tool's
        // launcher is a symbolic link, which Windows does not make, and a Windows tool's is a
        // `.cmd`.
        let windows = cfg!(windows);
        let platform = if windows {
            Platform::WindowsX64
        } else {
            Platform::LinuxX64
        };
        let program = if windows {
            "dist/fake.exe"
        } else {
            "dist/fake"
        };
        let root = scratch("release");
        let archive = zip_of(&[(program, b"#!/bin/sh\necho hi\n")]);
        let url = "https://example.test/fake.zip";
        let tool = tool_from(Asset {
            platform,
            url,
            sha256: sha256(&archive),
            archive: Archive::Zip,
            program,
        });
        let fetch = Fake(HashMap::from([(url.to_string(), archive)]));
        let mut lines = Vec::new();
        install(&root, &tool, platform, &fetch, &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert!(installed(&root, &tool, windows));
        assert!(
            std::fs::symlink_metadata(launcher(&root, "fake", windows)).is_ok(),
            "on PATH as `fake`"
        );
        assert!(
            !root.join("downloads/fake.zip.part").exists(),
            "the download is cleaned up"
        );
        assert!(lines.last().unwrap().starts_with("installed fake"));
        // Asking again is not a second download.
        install(&root, &tool, platform, &Fake(HashMap::new()), &mut |_| {}).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_download_that_does_not_match_its_pinned_hash_is_refused_and_removed() {
        // The reason the hashes are compiled in: a replaced asset must not be installed however
        // it was replaced.
        let root = scratch("tampered");
        let url = "https://example.test/fake.zip";
        let tool = tool_from(Asset {
            platform: Platform::LinuxX64,
            url,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            archive: Archive::Zip,
            program: "fake",
        });
        let fetch = Fake(HashMap::from([(
            url.to_string(),
            zip_of(&[("fake", b"evil")]),
        )]));
        let why = install(&root, &tool, Platform::LinuxX64, &fetch, &mut |_| {}).unwrap_err();
        assert!(why.contains("checksum"), "{why}");
        assert!(!installed(&root, &tool, false));
        assert!(!root.join("pkgs/fake-1.0").exists(), "nothing unpacked");
        assert!(
            !root.join("downloads/fake.zip.part").exists(),
            "and the file is gone"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_archive_entry_that_climbs_out_of_its_folder_is_refused() {
        let root = scratch("climb");
        let archive = root.join("evil.zip");
        std::fs::write(&archive, zip_of(&[("../../escaped", b"x")])).unwrap();
        let into = root.join("a/b");
        std::fs::create_dir_all(&into).unwrap();
        assert!(unpack(&archive, Archive::Zip, &into, "x").is_err());
        assert!(!root.join("escaped").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_single_gzipped_program_is_written_under_its_own_name() {
        let root = scratch("gz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"binary").unwrap();
        let archive = root.join("ra.gz");
        std::fs::write(&archive, encoder.finish().unwrap()).unwrap();
        unpack(&archive, Archive::Gz, &root, "rust-analyzer").unwrap();
        assert_eq!(
            std::fs::read(root.join("rust-analyzer")).unwrap(),
            b"binary"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_tool_with_no_build_for_this_machine_says_so_before_downloading() {
        let root = scratch("unavailable");
        let clangd = find("clangd").unwrap();
        let why = install(
            &root,
            clangd,
            Platform::LinuxArm64,
            &Fake(HashMap::new()),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(why.contains("ARM64 Linux") && why.contains("PATH"), "{why}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn launchers_name_their_program_in_full_on_each_platform() {
        // An npm tool's launcher names DaCode's own Node, so it runs on a machine with no `node`;
        // a Windows launcher is a `.cmd`, which `dacode_lsp::resolve` finds through PATHEXT.
        let node = Launch::Node {
            node: PathBuf::from("/t/node/bin/node"),
            entry: PathBuf::from("/t/p/it's.js"),
        };
        assert_eq!(
            launcher_text(&node, false).unwrap(),
            "#!/bin/sh\nexec '/t/node/bin/node' '/t/p/it'\\''s.js' \"$@\"\n"
        );
        let exe = Launch::Program(PathBuf::from(r"C:\t\ruff.exe"));
        assert_eq!(
            launcher_text(&exe, true).unwrap(),
            "@\"C:\\t\\ruff.exe\" %*\r\n"
        );
        assert_eq!(
            launcher_text(&Launch::Program(PathBuf::from("/t/ruff")), false),
            None
        );
        assert!(launcher(Path::new("/t"), "ruff", true).ends_with("ruff.cmd"));
    }

    #[test]
    fn npm_is_found_where_each_platforms_node_keeps_it() {
        assert_eq!(
            npm_cli(Path::new("/n/node-v1-linux-x64/bin/node"), false),
            Path::new("/n/node-v1-linux-x64/lib/node_modules/npm/bin/npm-cli.js")
        );
        assert_eq!(
            npm_cli(Path::new("/n/node-v1-win-x64/node.exe"), true),
            Path::new("/n/node-v1-win-x64/node_modules/npm/bin/npm-cli.js")
        );
    }

    #[test]
    fn an_npm_packages_bin_is_found_whether_it_is_one_script_or_a_map() {
        let folder = scratch("npm");
        for (package, bin) in [
            ("one", r#""bin": "cli.js""#),
            (
                "many",
                r#""bin": {"many-server": "server.js", "many": "cli.js"}"#,
            ),
        ] {
            let dir = folder.join("node_modules").join(package);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("package.json"), format!("{{ {bin} }}")).unwrap();
        }
        assert!(
            npm_entry(&folder, "one", "one")
                .unwrap()
                .ends_with("one/cli.js")
        );
        assert!(
            npm_entry(&folder, "many", "many-server")
                .unwrap()
                .ends_with("many/server.js")
        );
        assert!(npm_entry(&folder, "many", "absent").is_err());
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn the_launchers_go_on_the_end_of_path_once() {
        // Last, so a server the user installed is found before DaCode's; once, so restarting the
        // editor from its own terminal does not grow PATH every time.
        let root = Path::new("/state/tools");
        let joined = path_with_tools(Some(std::ffi::OsStr::new("/usr/bin")), root).unwrap();
        let parts: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(parts, vec![PathBuf::from("/usr/bin"), bin_dir(root)]);
        assert_eq!(path_with_tools(Some(&joined), root), None);
    }
}

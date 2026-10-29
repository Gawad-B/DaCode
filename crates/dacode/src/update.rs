//! Updating DaCode from its own GitHub releases.
//!
//! # What it does
//!
//! At most once a day, and whenever `check_for_updates` is run, a worker thread asks GitHub for the
//! latest release and compares its version with this build's. What happens next depends on how this
//! copy was installed, which is read from where the running executable is ([`install_kind`]):
//!
//! - **`install.sh` into a prefix under the home directory** (the tarball's layout, `~/.local` by
//!   default) updates itself: the tarball is downloaded to the state directory, checked against the
//!   release's `SHA256SUMS`, unpacked, and on "Restart now" its own `install.sh` is run into the
//!   same prefix and the new binary started with the same arguments.
//! - **A system package** (`/usr/bin`) is never touched — writing there needs root, and a package
//!   manager that finds its files changed behind it is worse off than one that is told. The
//!   notification gives the exact command for the downloaded `.deb` or `.rpm` instead.
//! - **The Windows installer's per-user directory** downloads `dacode-setup.exe`, checks it, and on
//!   "Restart now" runs it silently and quits; Inno Setup replaces the files.
//! - **A cargo build** (anything under a `target` directory, or `~/.cargo/bin`) never offers. The
//!   person running one updates it with `git pull`, and an editor that replaced a developer's build
//!   with a release would be very confusing.
//!
//! # Why curl, and why SHA-256 is written out here
//!
//! An HTTP client with TLS would be the largest dependency in the editor for one request a day.
//! `curl` is on every Linux desktop and has shipped with Windows since 10 (1803), so it is spawned
//! through [`crate::process::command_at`] like git is, and when it is missing the editor says so.
//!
//! The digest is FIPS 180-4 in about seventy lines below, checked against the standard's own
//! vectors. The `sha2` crate is small, but it would be a new dependency tree for a function this
//! size; shelling out would mean `sha256sum` here and `certutil` on Windows, two output formats to
//! parse, and a Windows half nothing on this machine could run.
//!
//! # What the check is worth
//!
//! There is no code signing. Integrity rests on HTTPS to GitHub and on `SHA256SUMS` attached to the
//! *same* release — which catches a truncated or corrupted download, and a swapped asset, but not a
//! compromised release, since whoever could replace the tarball could replace the sums beside it. A
//! release without `SHA256SUMS` (everything before the updater) is refused for self-install, and
//! the notification links to it instead.

use crate::wake::Waker;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// Where releases are asked about. Public: the repository is.
pub const LATEST: &str = "https://api.github.com/repos/Gawad-B/DaCode/releases/latest";

/// The file a release carries its checksums in, one `sha256sum` line per asset.
pub const SUMS: &str = "SHA256SUMS";

/// How long a check is good for, in seconds.
pub const CHECK_EVERY: u64 = 24 * 60 * 60;

/// What the `updates` setting says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Check daily, download and verify in the background, and offer a restart.
    #[default]
    Automatic,
    /// Check daily and say so; download only when asked.
    Notify,
    /// Never check by itself. `check_for_updates` still works.
    Off,
}

impl Policy {
    pub const ALL: [Policy; 3] = [Policy::Automatic, Policy::Notify, Policy::Off];

    pub const fn name(self) -> &'static str {
        match self {
            Policy::Automatic => "automatic",
            Policy::Notify => "notify",
            Policy::Off => "off",
        }
    }

    pub fn from_name(name: &str) -> Option<Policy> {
        Policy::ALL.into_iter().find(|policy| policy.name() == name)
    }
}

// ---- versions -----------------------------------------------------------------------------------

/// A release version: `1.2.3`, with or without a leading `v`, with whether it is a pre-release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `1.2.3-rc.1`. Pre-releases are never offered, and a pre-release *build* is older than the
    /// release of the same number, as semver orders them.
    pub pre: bool,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        // Build metadata says nothing about order.
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, true),
            Some(_) => return None,
            None => (text, false),
        };
        let mut parts = core.split('.');
        let mut number = || -> Option<u64> { parts.next()?.parse().ok() };
        let version = Version {
            major: number()?,
            minor: number()?,
            patch: number()?,
            pre,
        };
        parts.next().is_none().then_some(version)
    }
}

/// Whether `latest` should be offered to a build that is `current`. A pre-release never is.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(latest), Some(current)) = (Version::parse(latest), Version::parse(current)) else {
        return false;
    };
    if latest.pre {
        return false;
    }
    let core = |v: Version| (v.major, v.minor, v.patch);
    core(latest) > core(current) || (core(latest) == core(current) && current.pre)
}

// ---- the release ---------------------------------------------------------------------------------

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// The parts of GitHub's release answer the updater reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the `v`: `1.1.0`.
    pub version: String,
    /// The release's page, for "Release notes".
    pub page: String,
    pub prerelease: bool,
    pub draft: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// Reads `GET /repos/…/releases/latest`.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("GitHub's answer was not JSON: {error}"))?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or_default();
    let tag = text("tag_name");
    if tag.is_empty() {
        // An error body — a rate limit, say — is JSON with a `message` and no tag.
        let message = text("message");
        return Err(if message.is_empty() {
            "GitHub's answer named no release".to_string()
        } else {
            format!("GitHub said: {message}")
        });
    }
    let assets = value
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    Some(Asset {
                        name: asset.get("name")?.as_str()?.to_string(),
                        url: asset.get("browser_download_url")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version: tag.strip_prefix('v').unwrap_or(tag).to_string(),
        page: text("html_url").to_string(),
        prerelease: value
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        draft: value
            .get("draft")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        assets,
    })
}

// ---- how this copy was installed ----------------------------------------------------------------

/// Which system package format this machine uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    Deb,
    Rpm,
}

/// How the running copy got where it is, which decides how it is updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// `install.sh` into `prefix`, which the user owns: updated in place.
    Tarball { prefix: PathBuf },
    /// `/usr/bin`, owned by the package manager: the notification gives the command.
    System,
    /// `%LOCALAPPDATA%\Programs\dacode`: the setup program is downloaded and run.
    WindowsInstaller,
    /// Built from source: never offered an update.
    Cargo,
    /// Anywhere else — `/opt`, `/usr/local`, a copied binary. Told, and given the release page.
    Unknown,
}

impl Install {
    /// Whether the editor can install the update itself.
    pub fn updates_itself(&self) -> bool {
        matches!(self, Install::Tarball { .. } | Install::WindowsInstaller)
    }
}

/// Reads an install from the running executable's path.
///
/// Every input is a parameter, so the Windows rules have tests that run on Linux. Paths are
/// compared as text with `/` separators — a Windows path on Linux is one long component, and
/// `Path::starts_with` would see nothing in it.
pub fn install_kind(
    exe: &Path,
    windows: bool,
    home: &Path,
    local_appdata: Option<&Path>,
) -> Install {
    let normal = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_string();
        if windows { text.to_lowercase() } else { text }
    };
    let exe_text = normal(exe);
    let segments: Vec<&str> = exe_text.split('/').collect();
    let file = segments.last().copied().unwrap_or_default();
    let dir = &segments[..segments.len().saturating_sub(1)];

    // Before anything else: a build in `~/.local/share/x/target/release` is still a build.
    if dir.contains(&"target") || dir.ends_with(&[".cargo", "bin"]) {
        return Install::Cargo;
    }
    let named = if windows { "dacode.exe" } else { "dacode" };
    if file != named {
        return Install::Unknown;
    }
    let parent = dir.join("/");
    if windows {
        let expected = local_appdata.map(|base| format!("{}/programs/dacode", normal(base)));
        return if expected.is_some_and(|expected| parent == expected) {
            Install::WindowsInstaller
        } else {
            Install::Unknown
        };
    }
    if parent == "/usr/bin" {
        return Install::System;
    }
    let home = normal(home);
    match parent.strip_suffix("/bin") {
        Some(prefix) if !home.is_empty() && prefix.starts_with(&format!("{home}/")) => {
            Install::Tarball {
                prefix: PathBuf::from(prefix),
            }
        }
        _ => Install::Unknown,
    }
}

/// Which package format the machine wants, from `/etc/os-release` first and from which package
/// tool exists after that — a derivative names its parent in `ID_LIKE`, and one that names
/// nothing recognisable still has `dpkg` or `rpm`.
pub fn package_family(os_release: Option<&str>, has: impl Fn(&str) -> bool) -> Option<Package> {
    const DEB: [&str; 6] = [
        "debian",
        "ubuntu",
        "linuxmint",
        "pop",
        "elementary",
        "raspbian",
    ];
    const RPM: [&str; 9] = [
        "fedora",
        "rhel",
        "centos",
        "rocky",
        "almalinux",
        "suse",
        "opensuse",
        "mageia",
        "amzn",
    ];
    let words: Vec<String> = os_release
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("ID=")
                .or_else(|| line.strip_prefix("ID_LIKE="))
        })
        .flat_map(|value| {
            value
                .trim_matches('"')
                .split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        })
        .collect();
    let any = |names: &[&str]| words.iter().any(|word| names.contains(&word.as_str()));
    if any(&DEB) {
        Some(Package::Deb)
    } else if any(&RPM) {
        Some(Package::Rpm)
    } else if has("dpkg") {
        Some(Package::Deb)
    } else if has("rpm") {
        Some(Package::Rpm)
    } else {
        None
    }
}

/// The asset an install of this kind updates from.
pub fn pick_asset<'a>(
    release: &'a Release,
    install: &Install,
    package: Option<Package>,
) -> Option<&'a Asset> {
    let version = &release.version;
    let wanted = |asset: &&Asset| match install {
        Install::Tarball { .. } => asset.name == format!("dacode-{version}-x86_64-linux.tar.gz"),
        Install::WindowsInstaller => asset.name == "dacode-setup.exe",
        Install::System => match package {
            Some(Package::Deb) => asset.name == format!("dacode_{version}_amd64.deb"),
            Some(Package::Rpm) => {
                asset.name.starts_with(&format!("dacode-{version}-"))
                    && asset.name.ends_with(".x86_64.rpm")
            }
            None => false,
        },
        Install::Cargo | Install::Unknown => false,
    };
    release.assets.iter().find(wanted)
}

/// The command that installs a downloaded system package, as the notification prints it.
pub fn install_command(package: Package, asset: &str) -> String {
    match package {
        Package::Deb => format!("sudo apt install ./{asset}"),
        Package::Rpm => format!("sudo dnf install ./{asset}"),
    }
}

// ---- checksums ----------------------------------------------------------------------------------

/// Reads `sha256sum` output: `<64 hex>  <name>`, or `<hex> *<name>` in binary mode. Lines that are
/// not one of those are skipped; a file with none is an error.
pub fn parse_sums(text: &str) -> Result<Vec<(String, String)>, String> {
    let sums: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let (hash, name) = line.trim_end().split_once(char::is_whitespace)?;
            let name = name.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name);
            let valid = hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit());
            (valid && !name.is_empty()).then(|| (hash.to_ascii_lowercase(), name.to_string()))
        })
        .collect();
    if sums.is_empty() {
        Err(format!("{SUMS} lists no files"))
    } else {
        Ok(sums)
    }
}

/// The expected hash of `name`, if the sums list it.
pub fn expected<'a>(sums: &'a [(String, String)], name: &str) -> Option<&'a str> {
    sums.iter()
        .find(|(_, listed)| listed == name)
        .map(|(hash, _)| hash.as_str())
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256, FIPS 180-4, fed in pieces of any size.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }
}

impl Sha256 {
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        self.compress();
        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self) {
        let mut w = [0u32; 64];
        for (i, word) in self.block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The lowercase hex SHA-256 of a file, read in pieces so a large one is not held in memory.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::default();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finish()))
}

// ---- the daily gate and the skipped version -----------------------------------------------------

/// Whether a check is due, given when the last one was (seconds since the epoch). A clock that went
/// backwards counts as due rather than as "checked in the future", which would never come round.
pub fn due(last: Option<u64>, now: u64) -> bool {
    match last {
        None => true,
        Some(last) => now < last || now - last >= CHECK_EVERY,
    }
}

const STAMP: &str = "last-check";
const SKIPPED: &str = "skipped";

/// When the last check ran, from the updates directory.
pub fn last_check(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join(STAMP))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn record_check(dir: &Path, now: u64) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(STAMP), now.to_string());
}

/// The version "Skip this version" was last pressed for.
pub fn skipped(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(SKIPPED)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

pub fn skip(dir: &Path, version: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(SKIPPED), version)
}

pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

// ---- fetching -----------------------------------------------------------------------------------

/// How the worker reaches the network. A trait so a test hands it a fake and never touches it.
pub trait Fetch: Send {
    /// A small text answer: the release JSON.
    fn text(&self, url: &str) -> Result<String, String>;
    /// A file, written to `to`.
    fn file(&self, url: &str, to: &Path) -> Result<(), String>;
}

/// What is said when there is no curl to ask with.
pub const NO_CURL: &str = "checking for updates needs curl, which is not installed here — \
                           install curl, or download new releases from \
                           github.com/Gawad-B/DaCode/releases";

/// [`Fetch`] through the `curl` program.
pub struct Curl;

impl Curl {
    /// `seconds` bounds the whole request; `None` bounds only a stall, for a download whose size
    /// says nothing about how long it may honestly take.
    fn run(
        &self,
        url: &str,
        extra: &[&std::ffi::OsStr],
        seconds: Option<u32>,
    ) -> Result<Vec<u8>, String> {
        let Some(curl) = dacode_lsp::resolve("curl") else {
            return Err(NO_CURL.to_string());
        };
        let mut command = crate::process::command_at(&curl);
        // HTTPS only, redirects included: GitHub sends an asset download to its object store, and
        // a redirect that downgraded would be followed silently otherwise.
        command
            .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https"])
            .args(match seconds {
                Some(seconds) => vec!["--max-time".to_string(), seconds.to_string()],
                // Under a kilobyte a second for a whole minute is a stall. A fixed ceiling was
                // fifteen minutes, which a 118 MB clangd on a slow line runs straight into.
                None => vec![
                    "--speed-limit".to_string(),
                    "1024".to_string(),
                    "--speed-time".to_string(),
                    "60".to_string(),
                ],
            })
            .args(["-H", "Accept: application/vnd.github+json"])
            .args([
                "-H",
                &format!("User-Agent: DaCode/{}", env!("CARGO_PKG_VERSION")),
            ])
            .args(extra)
            .arg(url);
        let output = command
            .output()
            .map_err(|error| format!("could not run curl: {error}"))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let said = String::from_utf8_lossy(&output.stderr);
            let said = said.lines().last().unwrap_or("it gave no reason").trim();
            Err(format!("could not reach GitHub — {said}"))
        }
    }
}

impl Fetch for Curl {
    fn text(&self, url: &str) -> Result<String, String> {
        let bytes = self.run(url, &[], Some(30))?;
        String::from_utf8(bytes).map_err(|_| "GitHub's answer was not text".to_string())
    }

    fn file(&self, url: &str, to: &Path) -> Result<(), String> {
        self.run(url, &["-o".as_ref(), to.as_os_str()], None)
            .map(|_| ())
    }
}

// ---- staging ------------------------------------------------------------------------------------

/// A downloaded, verified update waiting for a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub version: String,
    /// The unpacked tarball's `install.sh`, or `dacode-setup.exe`.
    pub installer: PathBuf,
}

/// What is said when a release has no checksums to check against.
pub fn no_sums(version: &str) -> String {
    format!(
        "DaCode {version} has no {SUMS}, so it will not be installed automatically — download it \
         from the release page"
    )
}

/// Downloads `asset` into `dir/<version>`, checks it against the release's sums, and for the
/// tarball unpacks it. Runs on the worker thread.
pub fn stage(
    fetch: &dyn Fetch,
    release: &Release,
    asset: &Asset,
    dir: &Path,
) -> Result<Staged, String> {
    let Some(sums_asset) = release.asset(SUMS) else {
        return Err(no_sums(&release.version));
    };
    let here = dir.join(&release.version);
    let _ = std::fs::remove_dir_all(&here);
    std::fs::create_dir_all(&here)
        .map_err(|error| format!("could not make {}: {error}", here.display()))?;

    let sums_path = here.join(SUMS);
    fetch.file(&sums_asset.url, &sums_path)?;
    let sums = std::fs::read_to_string(&sums_path)
        .map_err(|error| format!("could not read {SUMS}: {error}"))
        .and_then(|text| parse_sums(&text))?;
    let Some(wanted) = expected(&sums, &asset.name) else {
        return Err(format!(
            "{SUMS} for DaCode {} does not list {}, so it was not installed",
            release.version, asset.name
        ));
    };

    // Downloaded under another name and renamed once it checks out, so nothing ever finds a
    // half-written file under the real one.
    let part = here.join(format!("{}.part", asset.name));
    fetch.file(&asset.url, &part)?;
    let actual =
        sha256_file(&part).map_err(|error| format!("could not read the download: {error}"))?;
    if actual != wanted {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "the download of DaCode {} did not match its {SUMS} entry, so it was thrown away \
             rather than installed",
            release.version
        ));
    }
    let file = here.join(&asset.name);
    std::fs::rename(&part, &file)
        .map_err(|error| format!("could not keep the download: {error}"))?;

    if !asset.name.ends_with(".tar.gz") {
        return Ok(Staged {
            version: release.version.clone(),
            installer: file,
        });
    }
    let unpacked = here.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|error| error.to_string())?;
    let output = crate::process::command("tar")
        .arg("-xzf")
        .arg(&file)
        .arg("-C")
        .arg(&unpacked)
        .output()
        .map_err(|error| format!("could not run tar to unpack the update: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not unpack DaCode {}: {}",
            release.version,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let installer = unpacked
        .join(format!("dacode-{}-x86_64-linux", release.version))
        .join("install.sh");
    if !installer.is_file() {
        return Err(format!(
            "DaCode {}'s tarball has no install.sh where it should be, so it was not installed",
            release.version
        ));
    }
    Ok(Staged {
        version: release.version.clone(),
        installer,
    })
}

/// How to start the editor again once the new one is in place.
#[derive(Debug, Clone)]
pub struct Relaunch {
    pub args: Vec<std::ffi::OsString>,
    pub dir: PathBuf,
}

/// The arguments the Windows installer is run with: no wizard, no questions, no reboot, and the
/// running editor closed if it has not finished quitting.
pub const SETUP_ARGS: [&str; 4] = [
    "/SILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/CLOSEAPPLICATIONS",
];

/// Puts a staged update in place and starts it. On the UI thread, at quit, after the session and
/// every unsaved buffer's recovery copy have been written.
///
/// It runs the release's own `install.sh`, which is a few file copies and two cache refreshes — the
/// one bounded thing the editor waits on as it leaves, said here rather than hidden. `install`
/// unlinks the old binary before writing the new one, so the running file is never written over.
/// If it fails nothing has been started and the editor stays open, still the old version.
pub fn apply(
    staged: &Staged,
    install: &Install,
    relaunch: Option<&Relaunch>,
) -> Result<(), String> {
    match install {
        Install::Tarball { prefix } => {
            let output = crate::process::command("sh")
                .arg(&staged.installer)
                .arg(prefix)
                .output()
                .map_err(|error| format!("could not run the update's install.sh: {error}"))?;
            if !output.status.success() {
                let said = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "DaCode {} could not be installed — {}. This version is still the one \
                     installed",
                    staged.version,
                    said.lines().last().unwrap_or("install.sh failed").trim()
                ));
            }
            if let Some(relaunch) = relaunch {
                crate::process::command_at(&prefix.join("bin").join("dacode"))
                    .args(&relaunch.args)
                    .current_dir(&relaunch.dir)
                    .spawn()
                    .map_err(|error| {
                        format!(
                            "DaCode {} is installed, but could not be started: {error} — start \
                             it again yourself",
                            staged.version
                        )
                    })?;
            }
            Ok(())
        }
        Install::WindowsInstaller => crate::process::command_at(&staged.installer)
            .args(SETUP_ARGS)
            .spawn()
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "could not start the DaCode {} setup: {error}",
                    staged.version
                )
            }),
        _ => Err("this copy of DaCode does not update itself".to_string()),
    }
}

// ---- the worker ---------------------------------------------------------------------------------

pub enum Request {
    /// `manual` when `check_for_updates` asked, so "you are up to date" is said only then.
    Check { manual: bool },
    Stage {
        release: Release,
        asset: Asset,
        dir: PathBuf,
    },
}

#[derive(Debug)]
pub enum Reply {
    /// The release if it is newer than this build, `None` if this build is current.
    Checked {
        manual: bool,
        result: Result<Option<Release>, String>,
    },
    Staged(Result<Staged, String>),
}

/// Checks and downloads off the UI thread, one request at a time.
pub struct Worker {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
    outstanding: std::cell::Cell<usize>,
}

impl Worker {
    /// `current` is the version to compare against — `CARGO_PKG_VERSION` in the editor, a chosen
    /// one in a test.
    pub fn spawn_with(fetch: Box<dyn Fetch>, current: &str, waker: Option<Waker>) -> Self {
        let (requests, incoming) = channel::<Request>();
        let (outgoing, replies) = channel::<Reply>();
        let current = current.to_string();
        std::thread::spawn(move || {
            while let Ok(request) = incoming.recv() {
                let reply = match request {
                    Request::Check { manual } => Reply::Checked {
                        manual,
                        result: fetch.text(LATEST).and_then(|json| {
                            let release = parse_release(&json)?;
                            Ok((!release.draft
                                && !release.prerelease
                                && is_newer(&release.version, &current))
                            .then_some(release))
                        }),
                    },
                    Request::Stage {
                        release,
                        asset,
                        dir,
                    } => Reply::Staged(stage(fetch.as_ref(), &release, &asset, &dir)),
                };
                if outgoing.send(reply).is_err() {
                    break;
                }
                if let Some(wake) = &waker {
                    wake();
                }
            }
        });
        Self {
            requests,
            replies,
            outstanding: std::cell::Cell::new(0),
        }
    }

    pub fn request(&self, request: Request) {
        self.outstanding.set(self.outstanding.get() + 1);
        let _ = self.requests.send(request);
    }

    pub fn busy(&self) -> bool {
        self.outstanding.get() > 0
    }

    /// The next reply, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<Reply> {
        let reply = self.replies.try_recv().ok()?;
        self.outstanding
            .set(self.outstanding.get().saturating_sub(1));
        Some(reply)
    }
}

// ---- what the bell shows -------------------------------------------------------------------------

/// Where an update on offer has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Found, and waiting to be asked for: the `notify` setting, or a system package.
    Available,
    Downloading,
    Ready(Staged),
    /// Cannot be installed from here; the sentence says why and what to do.
    Manual(String),
}

/// An update the editor knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub release: Release,
    pub stage: Stage,
}

/// A button on the offer: its words and the command it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub label: &'static str,
    pub command: &'static str,
}

const RESTART: Button = Button {
    label: "Restart now",
    command: "restart_to_update",
};
const NOTES: Button = Button {
    label: "Release notes",
    command: "update_release_notes",
};
const SKIP: Button = Button {
    label: "Skip this version",
    command: "skip_update",
};

/// What the bell's panel shows at its top while an update is on offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub message: String,
    pub buttons: Vec<Button>,
}

impl Pending {
    pub fn offer(&self, install: &Install, package: Option<Package>) -> Offer {
        let version = &self.release.version;
        match &self.stage {
            Stage::Ready(_) => Offer {
                message: format!("DaCode {version} is ready — restart to update"),
                buttons: vec![RESTART, NOTES, SKIP],
            },
            Stage::Downloading => Offer {
                message: format!("Downloading DaCode {version}…"),
                buttons: vec![NOTES],
            },
            Stage::Manual(why) => Offer {
                message: why.clone(),
                buttons: vec![NOTES, SKIP],
            },
            Stage::Available => {
                let asset = pick_asset(&self.release, install, package);
                let (message, first) = match (install, package, asset) {
                    (Install::System, Some(package), Some(asset)) => (
                        format!(
                            "DaCode {version} is available. Download it, then: {}",
                            install_command(package, &asset.name)
                        ),
                        "Download",
                    ),
                    _ => (format!("DaCode {version} is available"), "Update"),
                };
                Offer {
                    message,
                    buttons: vec![
                        Button {
                            label: first,
                            command: "download_update",
                        },
                        NOTES,
                        SKIP,
                    ],
                }
            }
        }
    }
}

/// What a found release becomes, given how this copy was installed and what the setting says.
pub fn found(
    release: Release,
    install: &Install,
    package: Option<Package>,
    policy: Policy,
) -> Option<Pending> {
    let version = release.version.clone();
    let stage = match install {
        Install::Cargo => return None,
        _ if install.updates_itself() => {
            if release.asset(SUMS).is_none() {
                Stage::Manual(no_sums(&version))
            } else if pick_asset(&release, install, package).is_none() {
                Stage::Manual(format!(
                    "DaCode {version} is available, but has no download for this platform"
                ))
            } else if policy == Policy::Automatic {
                Stage::Downloading
            } else {
                Stage::Available
            }
        }
        Install::System if pick_asset(&release, install, package).is_some() => Stage::Available,
        Install::System => Stage::Manual(format!(
            "DaCode {version} is available — download the .deb or .rpm from the release page"
        )),
        _ => Stage::Manual(format!(
            "DaCode {version} is available — this copy was not installed by install.sh or a \
             package, so update it the way it was installed"
        )),
    };
    Some(Pending { release, stage })
}

/// The command that opens a web page in the user's browser.
///
/// `rundll32 url.dll,FileProtocolHandler` on Windows rather than `cmd /c start`: `start` would read
/// the `&` in a URL as a command separator.
pub fn browser(url: &str, windows: bool) -> (&'static str, Vec<String>) {
    if windows {
        (
            "rundll32",
            vec!["url.dll,FileProtocolHandler".into(), url.into()],
        )
    } else {
        ("xdg-open", vec![url.into()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, names: &[&str]) -> Release {
        Release {
            version: version.into(),
            page: format!("https://github.com/Gawad-B/DaCode/releases/tag/v{version}"),
            prerelease: false,
            draft: false,
            assets: names
                .iter()
                .map(|name| Asset {
                    name: name.to_string(),
                    url: format!("https://example.invalid/{name}"),
                })
                .collect(),
        }
    }

    fn full(version: &str) -> Release {
        release(
            version,
            &[
                &format!("dacode-{version}-x86_64-linux.tar.gz"),
                &format!("dacode_{version}_amd64.deb"),
                &format!("dacode-{version}-1.x86_64.rpm"),
                "dacode-setup.exe",
                SUMS,
            ],
        )
    }

    #[test]
    fn a_higher_version_is_newer_in_every_place_and_only_there() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("v1.1.0", "1.0.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        // Compared as numbers: as text, 1.10 sorts before 1.9.
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn a_pre_release_is_never_offered_but_a_release_beats_its_own_pre_release() {
        assert!(!is_newer("1.1.0-rc.1", "1.0.0"));
        assert!(is_newer("1.1.0", "1.1.0-dev"));
        assert!(!is_newer("1.0.0+build.5", "1.0.0"));
    }

    #[test]
    fn a_version_that_is_not_three_numbers_is_not_compared() {
        // A tag like "nightly" must not read as newer than everything, or every copy would be
        // offered it forever.
        assert!(!is_newer("nightly", "1.0.0"));
        assert!(!is_newer("1.2", "1.0.0"));
        assert!(!is_newer("1.2.3.4", "1.0.0"));
        assert!(Version::parse("1.2.3-").is_none());
    }

    #[test]
    fn the_release_json_gives_the_version_page_and_assets() {
        let json = r#"{
            "tag_name": "v1.2.0", "html_url": "https://github.com/Gawad-B/DaCode/releases/tag/v1.2.0",
            "draft": false, "prerelease": false,
            "assets": [
                {"name": "SHA256SUMS", "browser_download_url": "https://x/SHA256SUMS", "size": 300},
                {"name": "dacode-setup.exe", "browser_download_url": "https://x/setup", "size": 9}
            ]
        }"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, "1.2.0");
        assert!(release.page.ends_with("v1.2.0"));
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.asset(SUMS).unwrap().url, "https://x/SHA256SUMS");
    }

    #[test]
    fn an_error_from_github_is_said_in_its_own_words() {
        // A rate limit is a JSON body with a message and no release; reporting "no tag" would hide
        // the one useful thing GitHub said.
        let error = parse_release(r#"{"message": "API rate limit exceeded"}"#).unwrap_err();
        assert!(error.contains("rate limit"), "{error}");
        assert!(parse_release("<html>").is_err());
    }

    #[test]
    fn each_install_is_read_from_where_the_executable_is() {
        let home = Path::new("/home/ada");
        let at = |path: &str| install_kind(Path::new(path), false, home, None);
        assert_eq!(
            at("/home/ada/.local/bin/dacode"),
            Install::Tarball {
                prefix: PathBuf::from("/home/ada/.local")
            }
        );
        assert_eq!(
            at("/home/ada/apps/bin/dacode"),
            Install::Tarball {
                prefix: PathBuf::from("/home/ada/apps")
            }
        );
        assert_eq!(at("/usr/bin/dacode"), Install::System);
        assert_eq!(at("/usr/local/bin/dacode"), Install::Unknown);
        assert_eq!(at("/opt/dacode/dacode"), Install::Unknown);
        // A home-directory prefix that merely starts with the same letters is not inside it.
        assert_eq!(at("/home/adam/.local/bin/dacode"), Install::Unknown);
    }

    #[test]
    fn a_build_is_never_mistaken_for_an_install_even_inside_the_home_directory() {
        let home = Path::new("/home/ada");
        for path in [
            "/home/ada/Coding/DaCode/target/release/dacode",
            "/home/ada/Coding/DaCode/target/debug/deps/dacode-1f2e",
            "/home/ada/.cargo/bin/dacode",
            // A checkout under ~/.local is still a checkout.
            "/home/ada/.local/src/DaCode/target/release/dacode",
        ] {
            assert_eq!(
                install_kind(Path::new(path), false, home, None),
                Install::Cargo
            );
        }
    }

    #[test]
    fn the_windows_installer_is_recognised_by_its_per_user_directory_from_linux() {
        let local = Path::new(r"C:\Users\Ada\AppData\Local");
        let at = |path: &str| {
            install_kind(
                Path::new(path),
                true,
                Path::new(r"C:\Users\Ada"),
                Some(local),
            )
        };
        assert_eq!(
            at(r"C:\Users\Ada\AppData\Local\Programs\dacode\dacode.exe"),
            Install::WindowsInstaller
        );
        // Case is not significant on Windows.
        assert_eq!(
            at(r"c:\users\ada\appdata\local\programs\DaCode\DACODE.EXE"),
            Install::WindowsInstaller
        );
        assert_eq!(at(r"C:\tools\dacode.exe"), Install::Unknown);
        assert_eq!(
            at(r"C:\src\DaCode\target\release\dacode.exe"),
            Install::Cargo
        );
    }

    #[test]
    fn the_package_format_comes_from_os_release_then_from_the_tools() {
        let none = |_: &str| false;
        let fedora = "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=44\n";
        assert_eq!(package_family(Some(fedora), none), Some(Package::Rpm));
        let mint = "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
        assert_eq!(package_family(Some(mint), none), Some(Package::Deb));
        // A derivative nobody listed still names its parent.
        let odd = "ID=nobara\nID_LIKE=\"fedora\"\n";
        assert_eq!(package_family(Some(odd), none), Some(Package::Rpm));
        // And with nothing recognisable, whichever tool exists.
        assert_eq!(
            package_family(Some("ID=mystery\n"), |tool| tool == "dpkg"),
            Some(Package::Deb)
        );
        assert_eq!(package_family(None, none), None);
    }

    #[test]
    fn each_install_picks_its_own_asset() {
        let release = full("1.2.0");
        let pick = |install: &Install, package| {
            pick_asset(&release, install, package).map(|asset| asset.name.clone())
        };
        let tarball = Install::Tarball {
            prefix: "/home/ada/.local".into(),
        };
        assert_eq!(
            pick(&tarball, None).as_deref(),
            Some("dacode-1.2.0-x86_64-linux.tar.gz")
        );
        assert_eq!(
            pick(&Install::WindowsInstaller, None).as_deref(),
            Some("dacode-setup.exe")
        );
        assert_eq!(
            pick(&Install::System, Some(Package::Deb)).as_deref(),
            Some("dacode_1.2.0_amd64.deb")
        );
        assert_eq!(
            pick(&Install::System, Some(Package::Rpm)).as_deref(),
            Some("dacode-1.2.0-1.x86_64.rpm")
        );
        assert_eq!(pick(&Install::System, None), None);
        assert_eq!(pick(&Install::Cargo, None), None);
        // An asset for another version is never picked, even with the right shape.
        let stale = release_with_tarball_of("1.1.0", "1.2.0");
        assert_eq!(pick_asset(&stale, &tarball, None), None);
    }

    fn release_with_tarball_of(asset_version: &str, version: &str) -> Release {
        release(
            version,
            &[&format!("dacode-{asset_version}-x86_64-linux.tar.gz"), SUMS],
        )
    }

    #[test]
    fn a_system_package_is_offered_with_the_command_that_installs_it() {
        let pending = found(
            full("1.2.0"),
            &Install::System,
            Some(Package::Rpm),
            Policy::Automatic,
        )
        .unwrap();
        // Never downloaded or installed from here, whatever the setting: /usr is root's.
        assert_eq!(pending.stage, Stage::Available);
        let offer = pending.offer(&Install::System, Some(Package::Rpm));
        assert!(
            offer
                .message
                .contains("sudo dnf install ./dacode-1.2.0-1.x86_64.rpm"),
            "{}",
            offer.message
        );
        assert_eq!(offer.buttons[0].label, "Download");
        let offer = pending.offer(&Install::System, Some(Package::Deb));
        assert!(
            offer
                .message
                .contains("sudo apt install ./dacode_1.2.0_amd64.deb")
        );
    }

    #[test]
    fn a_release_without_sums_is_refused_for_self_install() {
        let tarball = Install::Tarball {
            prefix: "/home/ada/.local".into(),
        };
        let old = release("1.2.0", &["dacode-1.2.0-x86_64-linux.tar.gz"]);
        let pending = found(old, &tarball, None, Policy::Automatic).unwrap();
        assert!(
            matches!(&pending.stage, Stage::Manual(why) if why.contains(SUMS)),
            "{:?}",
            pending.stage
        );
    }

    #[test]
    fn the_setting_decides_whether_a_found_update_downloads_at_once() {
        let tarball = Install::Tarball {
            prefix: "/home/ada/.local".into(),
        };
        let at = |policy| found(full("1.2.0"), &tarball, None, policy).unwrap().stage;
        assert_eq!(at(Policy::Automatic), Stage::Downloading);
        assert_eq!(at(Policy::Notify), Stage::Available);
        assert!(found(full("1.2.0"), &Install::Cargo, None, Policy::Automatic).is_none());
    }

    #[test]
    fn a_ready_update_offers_restart_notes_and_skip() {
        let pending = Pending {
            release: full("1.2.0"),
            stage: Stage::Ready(Staged {
                version: "1.2.0".into(),
                installer: "/x/install.sh".into(),
            }),
        };
        let offer = pending.offer(&Install::WindowsInstaller, None);
        assert_eq!(offer.message, "DaCode 1.2.0 is ready — restart to update");
        let labels: Vec<_> = offer.buttons.iter().map(|b| b.label).collect();
        assert_eq!(
            labels,
            ["Restart now", "Release notes", "Skip this version"]
        );
        // Every button runs a command the palette knows, or it is a button that does nothing.
        for button in offer.buttons {
            assert!(
                dacode_input::Action::from_name(button.command).is_some(),
                "{}",
                button.command
            );
        }
    }

    #[test]
    fn sha256_matches_the_standards_own_vectors() {
        let digest = |data: &[u8]| {
            let mut hasher = Sha256::default();
            hasher.update(data);
            hex(&hasher.finish())
        };
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 56 bytes: the length no longer fits in the first block, so padding takes a second.
        assert_eq!(
            digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_is_the_same_however_the_input_is_split() {
        // A million `a`s, fed in pieces that straddle every block boundary — the way a file is read.
        let mut hasher = Sha256::default();
        let piece = [b'a'; 997];
        let mut left = 1_000_000;
        while left > 0 {
            let take = left.min(piece.len());
            hasher.update(&piece[..take]);
            left -= take;
        }
        assert_eq!(
            hex(&hasher.finish()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn sha256sums_lines_are_read_in_text_and_binary_mode() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let text = format!("{a}  dacode-setup.exe\n{b} *SHA-ish.tar.gz\nnot a line\n");
        let sums = parse_sums(&text).unwrap();
        assert_eq!(expected(&sums, "dacode-setup.exe"), Some(a.as_str()));
        // Stored lowercase, which is what `hex` produces.
        assert_eq!(
            expected(&sums, "SHA-ish.tar.gz"),
            Some("b".repeat(64).as_str())
        );
        assert_eq!(expected(&sums, "missing"), None);
        assert!(parse_sums("nothing here\n").is_err());
    }

    #[test]
    fn a_check_is_due_once_a_day_and_after_the_clock_goes_back() {
        assert!(due(None, 1_000));
        assert!(!due(Some(1_000), 1_000 + CHECK_EVERY - 1));
        assert!(due(Some(1_000), 1_000 + CHECK_EVERY));
        assert!(
            due(Some(5_000), 1_000),
            "a stamp from the future is not trusted"
        );
    }

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dacode-update-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_stamp_and_the_skipped_version_live_in_the_updates_directory() {
        let dir = scratch("stamps").join("updates");
        assert_eq!(last_check(&dir), None);
        record_check(&dir, 1234);
        assert_eq!(last_check(&dir), Some(1234));
        assert_eq!(skipped(&dir), None);
        skip(&dir, "1.2.0").unwrap();
        assert_eq!(skipped(&dir).as_deref(), Some("1.2.0"));
    }

    /// Serves files from a map instead of the network.
    struct Fake(std::collections::HashMap<String, Vec<u8>>);

    impl Fetch for Fake {
        fn text(&self, url: &str) -> Result<String, String> {
            self.0
                .get(url)
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .ok_or_else(|| format!("no {url}"))
        }
        fn file(&self, url: &str, to: &Path) -> Result<(), String> {
            let bytes = self.0.get(url).ok_or_else(|| format!("no {url}"))?;
            std::fs::write(to, bytes).map_err(|error| error.to_string())
        }
    }

    fn sum_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::default();
        hasher.update(bytes);
        hex(&hasher.finish())
    }

    #[test]
    fn a_download_that_does_not_match_its_sum_is_thrown_away() {
        let dir = scratch("mismatch");
        let release = release("1.2.0", &["dacode-setup.exe", SUMS]);
        let mut files = std::collections::HashMap::new();
        files.insert(
            "https://example.invalid/dacode-setup.exe".to_string(),
            b"tampered".to_vec(),
        );
        files.insert(
            format!("https://example.invalid/{SUMS}"),
            format!("{}  dacode-setup.exe\n", sum_of(b"genuine")).into_bytes(),
        );
        let asset = release.asset("dacode-setup.exe").unwrap().clone();
        let error = stage(&Fake(files.clone()), &release, &asset, &dir).unwrap_err();
        assert!(error.contains("did not match"), "{error}");
        assert!(!dir.join("1.2.0").join("dacode-setup.exe").exists());
        assert!(!dir.join("1.2.0").join("dacode-setup.exe.part").exists());

        // And the genuine file is kept under its own name.
        files.insert(
            "https://example.invalid/dacode-setup.exe".to_string(),
            b"genuine".to_vec(),
        );
        let staged = stage(&Fake(files), &release, &asset, &dir).unwrap();
        assert_eq!(staged.installer, dir.join("1.2.0").join("dacode-setup.exe"));
    }

    #[test]
    fn a_tarball_is_staged_installed_into_its_prefix_and_left_alone_if_install_fails() {
        // `tar` and `sh` are what the Linux path runs; neither is something Windows guarantees.
        if cfg!(windows) {
            return;
        }
        let dir = scratch("apply");
        let version = "9.9.9";
        let top = format!("dacode-{version}-x86_64-linux");
        let source = dir.join("source").join(&top);
        std::fs::create_dir_all(source.join("bin")).unwrap();
        std::fs::write(source.join("bin").join("dacode"), "#!/bin/sh\necho new\n").unwrap();
        // The real install.sh's contract, small: copy bin/dacode under the prefix it is given.
        std::fs::write(
            source.join("install.sh"),
            "#!/bin/sh\nset -eu\nhere=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
             mkdir -p \"$1/bin\"\ncp \"$here/bin/dacode\" \"$1/bin/dacode\"\n",
        )
        .unwrap();
        let tarball = dir.join(format!("{top}.tar.gz"));
        let made = crate::process::command("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(dir.join("source"))
            .arg(&top)
            .status()
            .unwrap();
        assert!(made.success());
        let bytes = std::fs::read(&tarball).unwrap();

        let name = format!("{top}.tar.gz");
        let release = release(version, &[&name, SUMS]);
        let mut files = std::collections::HashMap::new();
        files.insert(format!("https://example.invalid/{name}"), bytes.clone());
        files.insert(
            format!("https://example.invalid/{SUMS}"),
            format!("{}  {name}\n", sum_of(&bytes)).into_bytes(),
        );
        let asset = release.asset(&name).unwrap().clone();
        let staged = stage(&Fake(files), &release, &asset, &dir.join("updates")).unwrap();
        assert!(staged.installer.ends_with("install.sh"));

        let prefix = dir.join("prefix");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::write(prefix.join("bin").join("dacode"), "old").unwrap();
        let install = Install::Tarball {
            prefix: prefix.clone(),
        };
        apply(&staged, &install, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(prefix.join("bin").join("dacode")).unwrap(),
            "#!/bin/sh\necho new\n"
        );

        // An install.sh that fails leaves the old copy in place and says so.
        std::fs::write(prefix.join("bin").join("dacode"), "old").unwrap();
        std::fs::write(
            &staged.installer,
            "#!/bin/sh\necho 'disk full' >&2\nexit 1\n",
        )
        .unwrap();
        let error = apply(&staged, &install, None).unwrap_err();
        assert!(error.contains("disk full"), "{error}");
        assert!(error.contains("still the one installed"), "{error}");
        assert_eq!(
            std::fs::read_to_string(prefix.join("bin").join("dacode")).unwrap(),
            "old"
        );
    }
}

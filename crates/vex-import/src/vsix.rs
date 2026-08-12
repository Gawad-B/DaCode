//! Reading `.vsix` extension packages.
//!
//! A `.vsix` is a zip archive with a `package.json` manifest. This module opens one, reads what it
//! *contributes*, and sorts those contributions into what vex can use and what it cannot — with a
//! reason attached to every refusal.
//!
//! **No JavaScript is executed, ever.** An extension's `main` entry point is inert here; it is
//! recorded as unsupported and ignored. That boundary is the whole design: the data comes across,
//! the code does not.

use crate::plan::ImportReport;
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::Path;

/// Something an extension contributes, classified by whether vex can use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// A color theme, with its path inside the archive.
    Theme { label: String, path: String },
    /// Snippets for a language.
    Snippets { language: String, path: String },
    /// Brackets, comments and auto-close pairs.
    LanguageConfig { language: String, path: String },
    /// A tree-sitter grammar. vex uses its own bundled grammars, so this is informational.
    Grammar { language: String },
    /// A language server vex could drive over LSP. The most valuable thing in most extensions.
    LanguageServer { hint: String },
    /// A debug adapter — usable over DAP once vex has debugging.
    DebugAdapter { kind: String },
    /// Something that needs the extension to actually run.
    RequiresJavaScript { what: String },
}

impl Capability {
    pub fn is_portable(&self) -> bool {
        !matches!(self, Capability::RequiresJavaScript { .. })
    }
}

/// A parsed extension package.
#[derive(Debug, Clone)]
pub struct Extension {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub publisher: String,
    pub capabilities: Vec<Capability>,
    /// Files in the archive, so contributions can be read without reopening it.
    files: std::collections::HashMap<String, Vec<u8>>,
}

impl Extension {
    /// The contents of a file inside the package.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        // Manifest paths are written relative to the extension root (`./themes/x.json`), while the
        // archive stores them under `extension/`. Try both spellings.
        let cleaned = path.trim_start_matches("./");
        self.files
            .get(cleaned)
            .or_else(|| self.files.get(&format!("extension/{cleaned}")))
            .map(Vec::as_slice)
    }

    pub fn json(&self, path: &str) -> Option<Value> {
        let bytes = self.file(path)?;
        let text = String::from_utf8_lossy(bytes);
        crate::parse_jsonc(&text).ok()
    }

    pub fn themes(&self) -> Vec<(&str, &str)> {
        self.capabilities
            .iter()
            .filter_map(|capability| match capability {
                Capability::Theme { label, path } => Some((label.as_str(), path.as_str())),
                _ => None,
            })
            .collect()
    }

    pub fn portable_count(&self) -> usize {
        self.capabilities
            .iter()
            .filter(|capability| capability.is_portable())
            .count()
    }
}

/// Opens a `.vsix` and classifies what it contains.
pub fn read_vsix(path: &Path, report: &mut ImportReport) -> Result<Extension> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid .vsix archive", path.display()))?;

    // Only files that could matter are kept in memory: a large extension is mostly compiled
    // JavaScript, which is of no use here.
    let mut files = std::collections::HashMap::new();
    let mut manifest_json: Option<Value> = None;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();

        let interesting = name.ends_with("package.json")
            || name.ends_with(".json")
            || name.ends_with(".jsonc")
            || name.ends_with("package.nls.json");
        if !interesting || entry.size() > 4 * 1024 * 1024 {
            continue;
        }

        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;

        if name == "extension/package.json" || name == "package.json" {
            let text = String::from_utf8_lossy(&bytes);
            manifest_json = crate::parse_jsonc(&text).ok();
        }
        files.insert(name, bytes);
    }

    let manifest = manifest_json.context("the package has no readable package.json manifest")?;
    let publisher = manifest
        .get("publisher")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let mut capabilities = Vec::new();

    if let Some(contributes) = manifest.get("contributes").and_then(Value::as_object) {
        if let Some(themes) = contributes.get("themes").and_then(Value::as_array) {
            for theme in themes {
                if let Some(path) = theme.get("path").and_then(Value::as_str) {
                    capabilities.push(Capability::Theme {
                        label: theme
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or("theme")
                            .to_string(),
                        path: path.to_string(),
                    });
                }
            }
        }

        if let Some(snippets) = contributes.get("snippets").and_then(Value::as_array) {
            for snippet in snippets {
                if let Some(path) = snippet.get("path").and_then(Value::as_str) {
                    capabilities.push(Capability::Snippets {
                        language: snippet
                            .get("language")
                            .and_then(Value::as_str)
                            .unwrap_or("*")
                            .to_string(),
                        path: path.to_string(),
                    });
                }
            }
        }

        if let Some(languages) = contributes.get("languages").and_then(Value::as_array) {
            for language in languages {
                let id = language
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                if let Some(path) = language.get("configuration").and_then(Value::as_str) {
                    capabilities.push(Capability::LanguageConfig {
                        language: id,
                        path: path.to_string(),
                    });
                }
            }
        }

        if let Some(grammars) = contributes.get("grammars").and_then(Value::as_array) {
            for grammar in grammars {
                capabilities.push(Capability::Grammar {
                    language: grammar
                        .get("language")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                });
            }
        }

        if let Some(debuggers) = contributes.get("debuggers").and_then(Value::as_array) {
            for adapter in debuggers {
                capabilities.push(Capability::DebugAdapter {
                    kind: adapter
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                });
            }
        }

        // Commands, views, menus and the like are the extension's *code* surface.
        for (key, label) in [
            ("commands", "commands"),
            ("views", "sidebar views"),
            ("viewsContainers", "view containers"),
            ("menus", "menu entries"),
            ("keybindings", "extension keybindings"),
        ] {
            if let Some(list) = contributes.get(key).and_then(Value::as_array) {
                if !list.is_empty() {
                    capabilities.push(Capability::RequiresJavaScript {
                        what: format!("{} {label}", list.len()),
                    });
                }
            }
        }
    }

    // A language-server extension is recognizable from its dependencies: it is a wrapper, and the
    // server it wraps is exactly the part vex wants.
    let dependencies = manifest
        .get("dependencies")
        .and_then(Value::as_object)
        .map(|deps| deps.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if dependencies
        .iter()
        .any(|dep| dep.contains("languageclient") || dep.contains("languageserver"))
    {
        capabilities.push(Capability::LanguageServer {
            hint: format!("{publisher}.{name} wraps a language server — configure it over LSP"),
        });
    }

    if manifest.get("main").is_some() {
        capabilities.push(Capability::RequiresJavaScript {
            what: "an activation entry point (extension code)".to_string(),
        });
    }

    let portable = capabilities.iter().filter(|c| c.is_portable()).count();
    if portable == 0 {
        report.note(format!(
            "{publisher}.{name} contributes nothing portable — it is entirely extension code"
        ));
    }

    Ok(Extension {
        id: format!("{publisher}.{name}"),
        display_name: manifest
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string(),
        version: manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0")
            .to_string(),
        publisher,
        capabilities,
        files,
    })
}

/// The Open VSX download URL for an extension.
///
/// Open VSX rather than Microsoft's marketplace: the latter's terms permit only Microsoft products,
/// so using it here would be a licence violation regardless of whether it worked.
pub fn open_vsx_url(publisher: &str, name: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!(
            "https://open-vsx.org/api/{publisher}/{name}/{version}/file/{publisher}.{name}-{version}.vsix"
        ),
        None => format!(
            "https://open-vsx.org/api/{publisher}/{name}/latest/file/{publisher}.{name}-latest.vsix"
        ),
    }
}

/// Downloads an extension from Open VSX using whichever fetcher is on the system.
///
/// Shelling out to `curl` or `wget` instead of linking an HTTP stack with TLS is deliberate: this
/// runs once, by hand, at import time. Adding rustls plus an async runtime to the editor binary to
/// serve a one-off operation would grow every startup and every byte of the download for no gain.
pub fn download_from_open_vsx(id: &str, destination: &Path) -> Result<()> {
    let (publisher, name) = id
        .split_once('.')
        .context("extension ids look like `publisher.name`")?;
    let url = open_vsx_url(publisher, name, None);

    for (program, args) in [("curl", vec!["-fsSL", "-o"]), ("wget", vec!["-q", "-O"])] {
        let mut command = std::process::Command::new(program);
        command.args(&args);
        command.arg(destination);
        command.arg(&url);

        match command.status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => {
                anyhow::bail!("{program} failed ({status}) fetching {url}");
            }
            // Not installed; try the next one.
            Err(_) => continue,
        }
    }

    anyhow::bail!(
        "neither curl nor wget is available — download {url} manually and pass it with --vsix"
    )
}

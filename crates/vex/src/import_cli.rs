//! The `vex import` command.
//!
//! Import is deliberately a separate, explicit operation rather than something that happens at
//! startup: it rewrites configuration, so it should happen when asked and its results should be
//! reviewable afterwards.
//!
//! Nothing is imported unless it is listed on the command line. `--list` shows what a source
//! offers without touching anything.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use vex_import::{
    Checklist, ImportItem, ImportReport, Outcome, import_keybindings, import_settings,
    import_snippets, import_theme, parse_jsonc, vscode_config_dir, vsix,
};

pub const HELP: &str = "\
vex import — bring settings, keybindings, themes and snippets over from VSCode

USAGE
    vex import [SOURCE] [WHAT...] [--out DIR]

SOURCE
    --from-vscode [DIR]   read a VSCode user directory (found automatically if omitted)
    --vsix FILE           read a .vsix extension package from disk
    --openvsx ID          download publisher.name from Open VSX, then read it

WHAT (nothing is imported unless listed)
    --settings            editor preferences from settings.json
    --keybindings         key bindings, where a vex equivalent exists
    --theme               color theme, converted to vex's perceptual palette
    --snippets            snippet definitions
    --language-config     brackets, comment tokens, auto-close pairs
    --language-server     a language server, to be driven over LSP
    --all                 everything the source offers

OTHER
    --list                show what the source offers, import nothing
    --out DIR             write to DIR instead of ~/.config/vex

NOTE
    VSCode extensions are JavaScript running against the `vscode` API, and vex has no JS runtime —
    by design, since that is what makes VSCode heavy. Extension *data* (themes, snippets, language
    configuration) is imported; extension *code* is reported as unsupported and skipped.

    Packages come from Open VSX, not Microsoft's marketplace, whose terms permit only Microsoft
    products.
";

struct Args {
    source: Source,
    checklist: Checklist,
    list_only: bool,
    out: PathBuf,
}

enum Source {
    VscodeDir(PathBuf),
    Vsix(PathBuf),
    OpenVsx(String),
}

pub fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return Ok(());
    }

    let parsed = parse_args(args)?;
    let mut report = ImportReport::default();

    match &parsed.source {
        Source::VscodeDir(dir) => import_from_vscode(dir, &parsed, &mut report)?,
        Source::Vsix(path) => import_from_vsix(path, &parsed, &mut report)?,
        Source::OpenVsx(id) => {
            // Download into the output directory so the package is kept: re-importing later should
            // not require the network again.
            std::fs::create_dir_all(&parsed.out).ok();
            let destination = parsed.out.join(format!("{id}.vsix"));
            eprintln!("vex: fetching {id} from Open VSX…");
            vsix::download_from_open_vsx(id, &destination)?;
            import_from_vsix(&destination, &parsed, &mut report)?;
        }
    }

    print!("{}", report.render());
    if report.had_failures() {
        anyhow::bail!("some items could not be imported");
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<Args> {
    let mut source = None;
    let mut checklist = Checklist::nothing();
    let mut list_only = false;
    let mut out = default_config_dir();

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let next = args.get(index + 1);

        match arg {
            "import" => {}
            "--list" => list_only = true,
            "--all" => checklist = Checklist::everything(),
            "--from-vscode" => {
                // The directory is optional: without it, look in the usual places.
                let dir = match next {
                    Some(value) if !value.starts_with("--") => {
                        index += 1;
                        PathBuf::from(value)
                    }
                    _ => vscode_config_dir()
                        .context("could not work out where VSCode keeps its configuration")?,
                };
                source = Some(Source::VscodeDir(dir));
            }
            "--vsix" => {
                let path = next.context("--vsix needs a path")?;
                index += 1;
                source = Some(Source::Vsix(PathBuf::from(path)));
            }
            "--openvsx" => {
                let id = next.context("--openvsx needs a publisher.name id")?;
                index += 1;
                source = Some(Source::OpenVsx(id.clone()));
            }
            "--out" => {
                let dir = next.context("--out needs a directory")?;
                index += 1;
                out = PathBuf::from(dir);
            }
            flag if flag.starts_with("--") => match ImportItem::parse(flag) {
                Some(item) => checklist = checklist.with(item),
                None => anyhow::bail!("unknown option {flag} (try `vex import --help`)"),
            },
            other => anyhow::bail!("unexpected argument {other} (try `vex import --help`)"),
        }
        index += 1;
    }

    let source = source.context(
        "no source given: use --from-vscode, --vsix FILE or --openvsx ID (try `vex import --help`)",
    )?;

    Ok(Args {
        source,
        checklist,
        list_only,
        out,
    })
}

fn default_config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config/vex")
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_jsonc(&text).ok()
}

fn import_from_vscode(dir: &Path, args: &Args, report: &mut ImportReport) -> Result<()> {
    eprintln!("vex: reading {}", dir.display());

    let settings_path = dir.join("settings.json");
    let keys_path = dir.join("keybindings.json");
    let snippets_dir = dir.join("snippets");

    if args.list_only {
        println!("{} offers:", dir.display());
        report_presence("settings", settings_path.exists());
        report_presence("keybindings", keys_path.exists());
        report_presence("snippets", snippets_dir.exists());
        println!(
            "\nselect what to import with flags, e.g.\n    vex import --from-vscode --settings --theme"
        );
        return Ok(());
    }

    if args.checklist.is_empty() {
        anyhow::bail!(
            "nothing selected — pass --settings, --theme, --keybindings, --snippets or --all"
        );
    }

    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    // Settings.
    if args.checklist.wants(ImportItem::Settings) {
        match read_json(&settings_path) {
            Some(json) => {
                let settings = import_settings(&json, report);
                let count = settings.count();
                let path = args.out.join("imported-settings.toml");
                std::fs::write(&path, settings.to_config())
                    .with_context(|| format!("writing {}", path.display()))?;
                report.note(format!("wrote {}", path.display()));
                report.record(ImportItem::Settings, Outcome::Imported { count });

                // The theme is named in settings but lives in an extension, so say so plainly
                // rather than silently doing nothing.
                if let Some(name) = settings.theme_name {
                    if args.checklist.wants(ImportItem::Theme) {
                        report.note(format!(
                            "theme {name:?} is provided by an extension — import it with \
                             `vex import --vsix <file> --theme`"
                        ));
                        report.record(ImportItem::Theme, Outcome::NotPresent);
                    }
                }
            }
            None => report.record(ImportItem::Settings, Outcome::NotPresent),
        }
    } else {
        report.record(ImportItem::Settings, Outcome::Skipped);
    }

    // Keybindings.
    if args.checklist.wants(ImportItem::Keybindings) {
        match read_json(&keys_path) {
            Some(json) => {
                let bindings = import_keybindings(&json, report);
                let mut out = String::from("# Imported from VSCode keybindings.json\n\n[keys]\n");
                for binding in &bindings {
                    out.push_str(&format!(
                        "{:?} = {:?}  # was {}\n",
                        binding.keys,
                        format!("{:?}", binding.action),
                        binding.source_command
                    ));
                }
                let path = args.out.join("imported-keys.toml");
                std::fs::write(&path, out)
                    .with_context(|| format!("writing {}", path.display()))?;
                report.note(format!("wrote {}", path.display()));
                report.record(
                    ImportItem::Keybindings,
                    Outcome::Imported {
                        count: bindings.len(),
                    },
                );
            }
            None => report.record(ImportItem::Keybindings, Outcome::NotPresent),
        }
    } else {
        report.record(ImportItem::Keybindings, Outcome::Skipped);
    }

    // Snippets: one file per language.
    if args.checklist.wants(ImportItem::Snippets) {
        let mut total = 0usize;
        if let Ok(entries) = std::fs::read_dir(&snippets_dir) {
            let target = args.out.join("snippets");
            std::fs::create_dir_all(&target).ok();

            for entry in entries.flatten() {
                let Some(json) = read_json(&entry.path()) else {
                    continue;
                };
                let snippets = import_snippets(&json, report);
                total += snippets.len();

                let name = entry.file_name().to_string_lossy().to_string();
                let mut out = format!("# Imported from {name}\n\n");
                for snippet in &snippets {
                    out.push_str(&format!(
                        "[[snippet]]\nprefix = {:?}\nbody = {:?}\ntab_stops = {:?}\n\n",
                        snippet.prefix, snippet.body, snippet.tab_stops
                    ));
                }
                let path = target.join(name.replace(".json", ".toml"));
                std::fs::write(&path, out).ok();
            }
        }
        if total == 0 {
            report.record(ImportItem::Snippets, Outcome::NotPresent);
        } else {
            report.record(ImportItem::Snippets, Outcome::Imported { count: total });
        }
    } else {
        report.record(ImportItem::Snippets, Outcome::Skipped);
    }

    Ok(())
}

fn import_from_vsix(path: &Path, args: &Args, report: &mut ImportReport) -> Result<()> {
    let extension = vsix::read_vsix(path, report)?;
    eprintln!(
        "vex: {} {} by {}",
        extension.display_name, extension.version, extension.publisher
    );

    if args.list_only {
        println!("{} offers:", extension.id);
        for capability in &extension.capabilities {
            let (mark, text) = match capability {
                vsix::Capability::Theme { label, .. } => ("+", format!("theme {label:?}")),
                vsix::Capability::Snippets { language, .. } => {
                    ("+", format!("snippets for {language}"))
                }
                vsix::Capability::LanguageConfig { language, .. } => {
                    ("+", format!("language configuration for {language}"))
                }
                vsix::Capability::Grammar { language } => (
                    "~",
                    format!("a TextMate grammar for {language} (vex uses tree-sitter)"),
                ),
                vsix::Capability::LanguageServer { hint } => ("+", hint.clone()),
                vsix::Capability::DebugAdapter { kind } => {
                    ("~", format!("a {kind} debug adapter (needs DAP support)"))
                }
                vsix::Capability::RequiresJavaScript { what } => (
                    "-",
                    format!("{what} — needs a JS runtime, which vex has none"),
                ),
            };
            println!("  {mark} {text}");
        }
        println!("\n  + portable   ~ informational   - not supported");
        return Ok(());
    }

    if args.checklist.is_empty() {
        anyhow::bail!("nothing selected — pass --theme, --snippets, --language-config or --all");
    }

    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    // Themes.
    if args.checklist.wants(ImportItem::Theme) {
        let themes = extension.themes();
        if themes.is_empty() {
            report.record(ImportItem::Theme, Outcome::NotPresent);
        } else {
            let mut imported = 0usize;
            let target = args.out.join("themes");
            std::fs::create_dir_all(&target).ok();

            for (label, theme_path) in themes {
                let Some(json) = extension.json(theme_path) else {
                    report.note(format!(
                        "theme {label:?} could not be read from the package"
                    ));
                    continue;
                };
                let (theme, recognized) = import_theme(&json, report);
                if recognized == 0 {
                    continue;
                }

                let file = target.join(format!("{}.toml", slug(label)));
                std::fs::write(&file, render_theme(label, &theme))
                    .with_context(|| format!("writing {}", file.display()))?;
                report.note(format!("wrote {}", file.display()));
                imported += 1;
            }
            report.record(ImportItem::Theme, Outcome::Imported { count: imported });
        }
    } else {
        report.record(ImportItem::Theme, Outcome::Skipped);
    }

    // Snippets.
    if args.checklist.wants(ImportItem::Snippets) {
        let mut total = 0usize;
        for capability in &extension.capabilities {
            if let vsix::Capability::Snippets {
                language,
                path: snippet_path,
            } = capability
            {
                if let Some(json) = extension.json(snippet_path) {
                    let snippets = import_snippets(&json, report);
                    total += snippets.len();
                    report.note(format!("{} snippets for {language}", snippets.len()));
                }
            }
        }
        if total == 0 {
            report.record(ImportItem::Snippets, Outcome::NotPresent);
        } else {
            report.record(ImportItem::Snippets, Outcome::Imported { count: total });
        }
    } else {
        report.record(ImportItem::Snippets, Outcome::Skipped);
    }

    // Language configuration and servers are recognized but not yet consumed by the editor, which
    // is stated rather than implied.
    if args.checklist.wants(ImportItem::LanguageConfig) {
        let count = extension
            .capabilities
            .iter()
            .filter(|c| matches!(c, vsix::Capability::LanguageConfig { .. }))
            .count();
        report.record(
            ImportItem::LanguageConfig,
            if count == 0 {
                Outcome::NotPresent
            } else {
                Outcome::Unsupported {
                    reason: format!(
                        "{count} found; vex has no auto-close or comment-toggle engine yet (M7)"
                    ),
                }
            },
        );
    }

    if args.checklist.wants(ImportItem::LanguageServer) {
        let servers: Vec<&str> = extension
            .capabilities
            .iter()
            .filter_map(|c| match c {
                vsix::Capability::LanguageServer { hint } => Some(hint.as_str()),
                _ => None,
            })
            .collect();
        report.record(
            ImportItem::LanguageServer,
            if servers.is_empty() {
                Outcome::NotPresent
            } else {
                Outcome::Unsupported {
                    reason: "an LSP client arrives at M6".to_string(),
                }
            },
        );
        for hint in servers {
            report.note(hint.to_string());
        }
    }

    // Always account for the code surface, selected or not: this is the boundary users most need
    // to understand.
    let js: Vec<&str> = extension
        .capabilities
        .iter()
        .filter_map(|c| match c {
            vsix::Capability::RequiresJavaScript { what } => Some(what.as_str()),
            _ => None,
        })
        .collect();
    if !js.is_empty() {
        report.note(format!(
            "skipped extension code: {} — vex runs no JavaScript",
            js.join(", ")
        ));
    }

    Ok(())
}

fn report_presence(what: &str, present: bool) {
    println!("  {} {what}", if present { "+" } else { "-" });
}

fn slug(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Writes a theme as vex config, in OKLCH so it stays inspectable and adjustable.
fn render_theme(label: &str, theme: &vex_theme::Theme) -> String {
    let color = |c: &vex_theme::Oklch| format!("\"oklch({:.3} {:.3} {:.1})\"", c.l, c.c, c.h);

    format!(
        "# Imported from a VSCode theme: {label}\n\
         [theme]\n\
         name = {label:?}\n\
         appearance = {:?}\n\n\
         [theme.surfaces]\n\
         bg = {}\n\
         bg_alt = {}\n\
         bg_panel = {}\n\
         row_active = {}\n\n\
         [theme.lines]\n\
         rule = {}\n\
         rule_soft = {}\n\n\
         [theme.text]\n\
         ink = {}\n\
         ink_dim = {}\n\
         ink_faint = {}\n\n\
         [theme.syntax]\n\
         keyword = {}\n\
         type = {}\n\
         function = {}\n\
         attribute = {}\n\
         comment = {}\n\
         string = {}\n\
         number = {}\n\
         constant = {}\n",
        match theme.appearance {
            vex_theme::Appearance::Light => "light",
            vex_theme::Appearance::Dark => "dark",
        },
        color(&theme.bg),
        color(&theme.bg_alt),
        color(&theme.bg_panel),
        color(&theme.row_active),
        color(&theme.rule),
        color(&theme.rule_soft),
        color(&theme.ink),
        color(&theme.ink_dim),
        color(&theme.ink_faint),
        color(&theme.syntax.keyword),
        color(&theme.syntax.type_),
        color(&theme.syntax.function),
        color(&theme.syntax.attribute),
        color(&theme.syntax.comment),
        color(&theme.syntax.string),
        color(&theme.syntax.number),
        color(&theme.syntax.constant),
    )
}

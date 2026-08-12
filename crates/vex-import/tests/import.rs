//! Import tests, against input shaped like real VSCode configuration.

use vex_import::{
    Capability, Checklist, ImportItem, ImportReport, Outcome, import_keybindings, import_settings,
    import_snippets, import_theme, parse_jsonc, vsix,
};
use vex_input::Action;
use vex_theme::Appearance;

#[test]
fn jsonc_comments_and_trailing_commas_are_tolerated() {
    // VSCode's own settings.json ships with comments, so rejecting them would fail on almost every
    // real configuration.
    let source = r##"{
        // line comment
        "editor.fontSize": 14, /* block comment */
        "editor.tabSize": 2,
    }"##;

    let json = parse_jsonc(source).expect("parses");
    assert_eq!(json["editor.fontSize"], 14);
    assert_eq!(json["editor.tabSize"], 2);
}

#[test]
fn a_slash_inside_a_string_is_not_a_comment() {
    let json = parse_jsonc(r##"{"path": "https://example.com/x", "b": 1}"##).expect("parses");
    assert_eq!(json["path"], "https://example.com/x");
    assert_eq!(json["b"], 1);
}

#[test]
fn settings_map_onto_vex_equivalents() {
    let source = r##"{
        "editor.fontSize": 15,
        "editor.fontFamily": "'JetBrains Mono', Menlo, monospace",
        "editor.tabSize": 2,
        "editor.insertSpaces": true,
        "editor.wordWrap": "on",
        "editor.rulers": [100],
        "editor.lineNumbers": "relative",
        "editor.formatOnSave": true,
        "workbench.colorTheme": "Solarized Light"
    }"##;

    let mut report = ImportReport::default();
    let settings = import_settings(&parse_jsonc(source).unwrap(), &mut report);

    assert_eq!(settings.code_size, Some(15.0));
    assert_eq!(
        settings.font_family.as_deref(),
        Some("JetBrains Mono"),
        "a CSS-style fallback list resolves to its first family"
    );
    assert_eq!(settings.tab_width, Some(2));
    assert_eq!(settings.insert_spaces, Some(true));
    assert_eq!(settings.soft_wrap, Some(true));
    assert_eq!(settings.ruler_column, Some(100));
    assert_eq!(settings.relative_line_numbers, Some(true));
    assert_eq!(settings.format_on_save, Some(true));
    assert_eq!(settings.theme_name.as_deref(), Some("Solarized Light"));
    assert_eq!(settings.prefers_light, Some(true));
}

#[test]
fn word_wrap_is_an_enum_not_a_boolean() {
    let mut report = ImportReport::default();
    let off = import_settings(
        &parse_jsonc(r##"{"editor.wordWrap": "off"}"##).unwrap(),
        &mut report,
    );
    assert_eq!(off.soft_wrap, Some(false));

    let bounded = import_settings(
        &parse_jsonc(r##"{"editor.wordWrap": "bounded"}"##).unwrap(),
        &mut report,
    );
    assert_eq!(bounded.soft_wrap, Some(true));
}

#[test]
fn rulers_may_be_objects_rather_than_numbers() {
    let mut report = ImportReport::default();
    let settings = import_settings(
        &parse_jsonc(r##"{"editor.rulers": [{"column": 80, "color": "#333"}]}"##).unwrap(),
        &mut report,
    );
    assert_eq!(settings.ruler_column, Some(80));
}

#[test]
fn unsupported_settings_are_reported_by_name_not_dropped_silently() {
    // A setting that vanished without explanation looks like a bug in vex.
    let mut report = ImportReport::default();
    import_settings(
        &parse_jsonc(r##"{"editor.minimap.enabled": true, "editor.someUnknownThing": 1}"##)
            .unwrap(),
        &mut report,
    );

    let notes = report.notes().join("\n");
    assert!(
        notes.contains("minimap"),
        "should name the minimap: {notes}"
    );
    assert!(
        notes.contains("no vex equivalent"),
        "should account for unknown keys: {notes}"
    );
}

#[test]
fn imported_settings_render_as_reviewable_config() {
    let mut report = ImportReport::default();
    let settings = import_settings(
        &parse_jsonc(r##"{"editor.fontSize": 16, "editor.tabSize": 8}"##).unwrap(),
        &mut report,
    );

    let config = settings.to_config();
    assert!(config.contains("font_size = 16"));
    assert!(config.contains("tab_width = 8"));
    assert!(config.starts_with("# Imported from VSCode"));
}

#[test]
fn keybindings_map_where_an_equivalent_exists() {
    let source = r##"[
        { "key": "ctrl+z", "command": "undo" },
        { "key": "ctrl+shift+z", "command": "redo" },
        { "key": "ctrl+s", "command": "workbench.action.files.save" },
        { "key": "alt+down", "command": "editor.action.insertCursorBelow" }
    ]"##;

    let mut report = ImportReport::default();
    let bindings = import_keybindings(&parse_jsonc(source).unwrap(), &mut report);

    assert_eq!(bindings.len(), 4);
    assert_eq!(bindings[0].keys, "<c-z>");
    assert_eq!(bindings[0].action, Action::Undo);
    assert_eq!(bindings[2].action, Action::Save);
    assert_eq!(bindings[3].action, Action::AddCaretBelow);
}

#[test]
fn unmappable_commands_and_chords_are_reported() {
    let source = r##"[
        { "key": "ctrl+k ctrl+d", "command": "undo" },
        { "key": "ctrl+alt+q", "command": "some.extension.command" },
        { "key": "ctrl+z", "command": "-undo" }
    ]"##;

    let mut report = ImportReport::default();
    let bindings = import_keybindings(&parse_jsonc(source).unwrap(), &mut report);

    assert!(bindings.is_empty(), "none of these should map");
    let notes = report.notes().join("\n");
    assert!(notes.contains("multi-chord"), "{notes}");
    assert!(notes.contains("no vex equivalent"), "{notes}");
}

#[test]
fn a_dark_theme_converts_and_keeps_its_colors() {
    let source = r##"{
        "type": "dark",
        "colors": {
            "editor.background": "#1e1e1e",
            "editor.foreground": "#d4d4d4",
            "editorLineNumber.foreground": "#858585"
        },
        "tokenColors": [
            { "scope": "keyword.control", "settings": { "foreground": "#c586c0" } },
            { "scope": ["comment"], "settings": { "foreground": "#6a9955" } },
            { "scope": "string", "settings": { "foreground": "#ce9178" } }
        ]
    }"##;

    let mut report = ImportReport::default();
    let (theme, recognized) = import_theme(&parse_jsonc(source).unwrap(), &mut report);

    assert_eq!(theme.appearance, Appearance::Dark);
    assert!(recognized >= 6, "recognized only {recognized} colors");
    assert_eq!(theme.bg.to_hex(), "#1e1e1e");
    assert_eq!(theme.ink.to_hex(), "#d4d4d4");
    assert_eq!(theme.syntax.keyword.to_hex(), "#c586c0");
    assert_eq!(theme.syntax.comment.to_hex(), "#6a9955");
    assert_eq!(theme.syntax.string.to_hex(), "#ce9178");
}

#[test]
fn appearance_is_inferred_from_the_background_when_unstated() {
    // A theme with a light background is a light theme, whatever it calls itself.
    let source =
        r##"{"colors": {"editor.background": "#fdf6e3", "editor.foreground": "#586e75"}}"##;
    let mut report = ImportReport::default();
    let (theme, _) = import_theme(&parse_jsonc(source).unwrap(), &mut report);
    assert_eq!(theme.appearance, Appearance::Light);
}

#[test]
fn the_most_specific_matching_scope_wins() {
    // Themes set both broad and narrow scopes; the narrow one is what the author meant.
    let source = r##"{
        "type": "dark",
        "colors": {"editor.background": "#000000"},
        "tokenColors": [
            { "scope": "keyword", "settings": { "foreground": "#111111" } },
            { "scope": "keyword.control.flow", "settings": { "foreground": "#222222" } }
        ]
    }"##;

    let mut report = ImportReport::default();
    let (theme, _) = import_theme(&parse_jsonc(source).unwrap(), &mut report);
    assert_eq!(theme.syntax.keyword.to_hex(), "#222222");
}

#[test]
fn an_illegible_import_is_corrected_and_reported() {
    // Importing a theme that renders text invisible would be worse than not importing it.
    let source = r##"{
        "type": "dark",
        "colors": {"editor.background": "#202020", "editor.foreground": "#252525"}
    }"##;

    let mut report = ImportReport::default();
    let (theme, _) = import_theme(&parse_jsonc(source).unwrap(), &mut report);

    assert!(
        (theme.ink.l - theme.bg.l).abs() > 0.3,
        "contrast should have been corrected: ink {} vs bg {}",
        theme.ink.l,
        theme.bg.l
    );
    assert!(report.notes().iter().any(|note| note.contains("contrast")));
}

#[test]
fn snippets_parse_including_their_tab_stops() {
    let source = r##"{
        "For Loop": {
            "prefix": "for",
            "body": ["for (let ${1:i} = 0; $1 < ${2:len}; $1++) {", "\t$0", "}"],
            "description": "A for loop"
        },
        "Plain": { "prefix": "plain", "body": "just text" }
    }"##;

    let mut report = ImportReport::default();
    let snippets = import_snippets(&parse_jsonc(source).unwrap(), &mut report);

    assert_eq!(snippets.len(), 2);
    let loop_snippet = snippets.iter().find(|s| s.prefix == "for").unwrap();
    assert_eq!(
        loop_snippet.tab_stops,
        vec![1, 2, 0],
        "$0 sorts last however it was written"
    );
    assert!(
        loop_snippet.body.contains('\n'),
        "array bodies join with newlines"
    );

    let plain = snippets.iter().find(|s| s.prefix == "plain").unwrap();
    assert!(!plain.has_placeholders());

    assert!(
        report
            .notes()
            .iter()
            .any(|note| note.contains("expansion is not implemented")),
        "the inertness of placeholders must be stated: {:?}",
        report.notes()
    );
}

#[test]
fn the_checklist_takes_nothing_by_default() {
    // The core guarantee: an import never touches something that was not selected.
    let nothing = Checklist::nothing();
    assert!(nothing.is_empty());
    for item in ImportItem::ALL {
        assert!(
            !nothing.wants(item),
            "{item} must not be selected by default"
        );
    }

    let some = Checklist::of([ImportItem::Theme, ImportItem::Settings]);
    assert!(some.wants(ImportItem::Theme));
    assert!(!some.wants(ImportItem::Keybindings));

    assert!(Checklist::everything().wants(ImportItem::Snippets));
}

#[test]
fn checklist_items_round_trip_through_their_flags() {
    for item in ImportItem::ALL {
        assert_eq!(ImportItem::parse(item.flag()), Some(item));
        assert_eq!(ImportItem::parse(&format!("--{}", item.flag())), Some(item));
    }
    assert_eq!(ImportItem::parse("nonsense"), None);
}

#[test]
fn the_report_states_what_happened_to_every_item() {
    let mut report = ImportReport::default();
    report.record(ImportItem::Settings, Outcome::Imported { count: 7 });
    report.record(ImportItem::Theme, Outcome::Skipped);
    report.record(ImportItem::Snippets, Outcome::NotPresent);
    report.record(
        ImportItem::LanguageServer,
        Outcome::Unsupported {
            reason: "LSP arrives at M6".into(),
        },
    );

    let rendered = report.render();
    assert!(rendered.contains("imported (7)"));
    assert!(rendered.contains("skipped"));
    assert!(rendered.contains("nothing to import"));
    assert!(rendered.contains("LSP arrives at M6"));
    assert_eq!(report.imported_count(), 7);
    assert!(!report.had_failures());
}

#[test]
fn open_vsx_is_the_registry_used() {
    // Microsoft's marketplace terms permit only Microsoft products, so the URL must not point there.
    let url = vsix::open_vsx_url("rust-lang", "rust-analyzer", Some("0.3.0"));
    assert!(url.starts_with("https://open-vsx.org/"), "{url}");
    assert!(!url.contains("visualstudio.com"), "{url}");
    assert!(url.contains("rust-lang.rust-analyzer-0.3.0.vsix"), "{url}");
}

#[test]
fn a_vsix_is_read_and_its_contributions_classified() {
    // Build a minimal .vsix in a temp dir: a zip with a package.json and a theme.
    let dir = std::env::temp_dir().join(format!("vex-import-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sample.vsix");

    let manifest = r##"{
        "publisher": "acme",
        "name": "sample",
        "displayName": "Sample",
        "version": "1.2.3",
        "main": "./out/extension.js",
        "contributes": {
            "themes": [{ "label": "Sample Dark", "path": "./themes/dark.json" }],
            "snippets": [{ "language": "rust", "path": "./snippets/rust.json" }],
            "languages": [{ "id": "rust", "configuration": "./language-configuration.json" }],
            "grammars": [{ "language": "rust" }],
            "commands": [{ "command": "sample.doThing", "title": "Do Thing" }]
        }
    }"##;
    let theme = r##"{"type": "dark", "colors": {"editor.background": "#101010"}}"##;

    {
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        use std::io::Write;
        writer
            .start_file("extension/package.json", options)
            .unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer
            .start_file("extension/themes/dark.json", options)
            .unwrap();
        writer.write_all(theme.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    let mut report = ImportReport::default();
    let extension = vsix::read_vsix(&path, &mut report).expect("reads the package");

    assert_eq!(extension.id, "acme.sample");
    assert_eq!(extension.version, "1.2.3");

    // Portable contributions are recognized...
    assert!(extension.capabilities.iter().any(|c| matches!(
        c,
        Capability::Theme { label, .. } if label == "Sample Dark"
    )));
    assert!(
        extension
            .capabilities
            .iter()
            .any(|c| matches!(c, Capability::Snippets { .. }))
    );
    assert!(
        extension
            .capabilities
            .iter()
            .any(|c| matches!(c, Capability::LanguageConfig { .. }))
    );

    // ...and the code surface is recorded as unsupported rather than ignored.
    let js: Vec<&Capability> = extension
        .capabilities
        .iter()
        .filter(|c| matches!(c, Capability::RequiresJavaScript { .. }))
        .collect();
    assert!(
        js.len() >= 2,
        "the entry point and the commands both need JS: {:?}",
        extension.capabilities
    );

    // The theme file is readable through the archive, resolving `./` paths.
    let theme_json = extension
        .json("./themes/dark.json")
        .expect("theme is readable");
    assert_eq!(theme_json["colors"]["editor.background"], "#101010");

    let (converted, _) = import_theme(&theme_json, &mut report);
    assert_eq!(converted.bg.to_hex(), "#101010");

    std::fs::remove_dir_all(&dir).ok();
}

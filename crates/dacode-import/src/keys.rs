//! `keybindings.json` → DaCode Actions.
//!
//! Only bindings whose command has a DaCode counterpart are mapped. The rest are reported by name:
//! most VSCode commands come from extensions and refer to behavior DaCode does not have, and quietly
//! dropping them would leave the user wondering why a key does nothing.

use crate::plan::ImportReport;
use dacode_input::{Action, InsertPosition, Motion, Operator, Target};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedKeybinding {
    /// The chord in DaCode notation: `<c-s>`, `dd`, `<esc>`.
    pub keys: String,
    pub action: Action,
    /// The VSCode command it came from, so the mapping is auditable.
    pub source_command: String,
}

/// Maps a VSCode command id onto an Action.
///
/// Deliberately conservative: a wrong mapping is worse than a missing one, because a key that does
/// the wrong thing destroys text while a key that does nothing merely disappoints.
fn map_command(command: &str) -> Option<Action> {
    Some(match command {
        "undo" => Action::Undo,
        "redo" => Action::Redo,
        "workbench.action.files.save" => Action::Save,
        "editor.action.selectAll" => Action::SelectAll,
        "editor.action.insertLineAfter" => Action::InsertAt(InsertPosition::LineBelow),
        "editor.action.insertLineBefore" => Action::InsertAt(InsertPosition::LineAbove),
        "editor.action.indentLines" => Action::Apply {
            operator: Operator::Indent,
            target: Target::WholeLine { count: 1 },
            register: None,
        },
        "editor.action.outdentLines" => Action::Apply {
            operator: Operator::Outdent,
            target: Target::WholeLine { count: 1 },
            register: None,
        },
        "editor.action.deleteLines" => Action::Apply {
            operator: Operator::Delete,
            target: Target::WholeLine { count: 1 },
            register: None,
        },
        "editor.action.transformToUppercase" => Action::Apply {
            operator: Operator::Uppercase,
            target: Target::Selection,
            register: None,
        },
        "editor.action.transformToLowercase" => Action::Apply {
            operator: Operator::Lowercase,
            target: Target::Selection,
            register: None,
        },
        "cursorHome" => Action::Move {
            motion: Motion::LineStart,
            count: 1,
        },
        "cursorEnd" => Action::Move {
            motion: Motion::LineEnd,
            count: 1,
        },
        "cursorTop" => Action::Move {
            motion: Motion::FileStart,
            count: 1,
        },
        "cursorBottom" => Action::Move {
            motion: Motion::FileEnd,
            count: 1,
        },
        "editor.action.addSelectionToNextFindMatch" => Action::AddCaretBelow,
        "editor.action.insertCursorBelow" => Action::AddCaretBelow,
        "editor.action.insertCursorAbove" => Action::AddCaretAbove,
        "workbench.action.toggleWordWrap" => Action::ToggleSoftWrap,
        "editor.action.fontZoomIn" | "workbench.action.zoomIn" => Action::IncreaseFontSize,
        "editor.action.fontZoomOut" | "workbench.action.zoomOut" => Action::DecreaseFontSize,
        "workbench.action.closeWindow" => Action::Quit,
        _ => return None,
    })
}

/// Translates VSCode's chord notation (`ctrl+shift+p`, `cmd+s`) into DaCode's (`<c-P>`, `<c-s>`).
///
/// Multi-chord sequences (`ctrl+k ctrl+d`) are rejected rather than half-translated: DaCode's grammar
/// has no chord-sequence concept yet, and a partial translation would bind the wrong key.
fn translate_chord(chord: &str) -> Option<String> {
    if chord.split_whitespace().count() > 1 {
        return None;
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut key = None;

    for part in chord.split('+') {
        match part.trim().to_lowercase().as_str() {
            // `cmd` is treated as ctrl: on Linux there is no super-key convention for editors.
            "ctrl" | "cmd" | "meta" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            other => key = Some(other.to_string()),
        }
    }

    let key = key?;
    let base = match key.as_str() {
        "escape" | "esc" => "<esc>".to_string(),
        "enter" | "return" => "<cr>".to_string(),
        "tab" => "<tab>".to_string(),
        "space" => "<space>".to_string(),
        "backspace" => "<bs>".to_string(),
        "delete" | "del" => "<del>".to_string(),
        "left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown" => {
            format!("<{key}>")
        }
        single if single.chars().count() == 1 => {
            let c = single.chars().next().unwrap();
            if shift {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        }
        _ => return None,
    };

    Some(match (ctrl, alt) {
        (true, _) => format!("<c-{}>", base.trim_matches(['<', '>'])),
        (_, true) => format!("<a-{}>", base.trim_matches(['<', '>'])),
        _ => base,
    })
}

pub fn import_keybindings(json: &Value, report: &mut ImportReport) -> Vec<ImportedKeybinding> {
    let Some(entries) = json.as_array() else {
        report.note("keybindings.json is not a JSON array");
        return Vec::new();
    };

    let mut imported = Vec::new();
    let mut unmapped_commands: Vec<String> = Vec::new();
    let mut unmapped_chords = 0usize;

    for entry in entries {
        let Some(command) = entry.get("command").and_then(Value::as_str) else {
            continue;
        };
        // A leading `-` removes a default binding; DaCode's defaults are its own, so there is nothing
        // to remove.
        if command.starts_with('-') {
            continue;
        }
        let Some(chord) = entry.get("key").and_then(Value::as_str) else {
            continue;
        };

        match (translate_chord(chord), map_command(command)) {
            (Some(keys), Some(action)) => imported.push(ImportedKeybinding {
                keys,
                action,
                source_command: command.to_string(),
            }),
            (None, _) => unmapped_chords += 1,
            (_, None) => {
                if !unmapped_commands.contains(&command.to_string()) {
                    unmapped_commands.push(command.to_string());
                }
            }
        }
    }

    if unmapped_chords > 0 {
        report.note(format!(
            "{unmapped_chords} bindings used multi-chord sequences, which DaCode does not support yet"
        ));
    }
    if !unmapped_commands.is_empty() {
        let shown: Vec<&str> = unmapped_commands
            .iter()
            .take(5)
            .map(String::as_str)
            .collect();
        report.note(format!(
            "{} commands have no DaCode equivalent (e.g. {})",
            unmapped_commands.len(),
            shown.join(", ")
        ));
    }

    imported
}

//! Every key the cheat sheet claims, fed through the real grammar.
use dacode_input::{Grammar, Key, Resolution, commands};
use dacode_theme::Mode;

fn resolves(mode: Mode, keys: &str) -> bool {
    let mut grammar = Grammar::new();
    let mut got = false;
    for key in Key::parse_sequence(keys) {
        match grammar.feed(mode, key) {
            Resolution::Action(_) => got = true,
            Resolution::Unhandled => return false,
            _ => {}
        }
    }
    got
}

/// The same, with the modal grammar switched off.
fn resolves_flat(keys: &str) -> bool {
    let mut grammar = Grammar::new();
    grammar.set_modal(false);
    let mut got = false;
    for key in Key::parse_sequence(keys) {
        match grammar.feed(Mode::Insert, key) {
            Resolution::Action(_) => got = true,
            Resolution::Unhandled => return false,
            _ => {}
        }
    }
    got
}

/// Every non-modal binding the command table advertises, through the real non-modal grammar.
///
/// The table is what the command palette and the help panel read, so an entry whose key does not
/// resolve is the editor telling a user to press something that does nothing — the one failure a
/// discoverability feature cannot survive. This is the check that makes the table trustworthy, and
/// it is why `to_display` and `parse_sequence` have to agree on how a key is spelled.
fn unresolved_flat_bindings() -> Vec<String> {
    let mut missing = Vec::new();
    for command in commands::COMMANDS {
        for keys in command.flat.split_whitespace() {
            if keys.is_empty() {
                continue;
            }
            if !resolves_flat(keys) {
                missing.push(format!("{} ({keys})", command.name));
            }
        }
    }
    missing
}

fn main() {
    let normal = [
        "i",
        "a",
        "I",
        "A",
        "o",
        "O",
        "v",
        "R",
        "h",
        "j",
        "k",
        "l",
        " ",
        "w",
        "W",
        "b",
        "B",
        "e",
        "E",
        "0",
        "^",
        "$",
        "{",
        "}",
        "%",
        "fx",
        "Fx",
        "tx",
        "Tx",
        "G",
        "gg",
        "ge",
        "gh",
        "gl",
        "dw",
        "cw",
        "yy",
        "dd",
        ">>",
        "<<",
        "Uiw",
        "Liw",
        "x",
        "rx",
        "p",
        "P",
        "u",
        "<c-r>",
        ".",
        "diw",
        "daw",
        "di\"",
        "di(",
        "di[",
        "di{",
        "di<",
        "dip",
        "dil",
        "daf",
        "cif",
        "yat",
        "dia",
        "dac",
        "gA",
        "C",
        "S",
        ",",
        "V",
        "g%",
        "/",
        "?",
        "n",
        "N",
        ":",
        "<c-e>",
        "<c-`>",
        "<c-s-e>",
        "<c-1>",
        "<c-k>o",
        "<c-k>s",
        "<c-,>",
        "<c-s>",
        "<c-q>",
        "<c-w>v",
        "<c-w>s",
        "<c-w>c",
        "<c-w>q",
        "<c-w>w",
        "<c-w>W",
        "<c-w>h",
        "<c-w>l",
        "<c-w>j",
        "<c-w>k",
        "K",
        "gd",
        "gu",
        "gs",
        "gS",
        "gr",
        "gf",
        "ga",
        "gi",
        "]d",
        "[d",
        "gD",
        "<c-space>",
        "za",
        "zc",
        "zo",
        "zR",
        "zM",
        "<c-o>",
        "<c-i>",
        "qa",
        "@a",
        "@@",
        "zw",
        "gt",
        "=",
        "+",
        "-",
    ];

    let mut missing = Vec::new();
    for keys in normal {
        if !resolves(Mode::Normal, keys) {
            missing.push(keys);
        }
    }
    // Insert mode: tab expands or indents.
    if !resolves(Mode::Insert, "<tab>") {
        missing.push("<tab> (insert)");
    }
    if !resolves(Mode::Insert, "<c-space>") {
        missing.push("<c-space> (insert)");
    }

    // ctrl-p, ctrl-b and ctrl-f are handled by the application rather than the grammar — they open
    // palettes and panels, which the grammar knows nothing about. The grammar must leave them alone
    // rather than treating them as plain letters, which is what `Unhandled` means here.
    for keys in ["<c-p>", "<c-b>", "<c-f>"] {
        if resolves(Mode::Normal, keys) {
            missing.push(keys);
        }
    }

    // Visual mode: a text object with no operator selects itself.
    for keys in ["af", "it", "aa", "ic", "iw"] {
        if !resolves(Mode::Visual, keys) {
            missing.push(keys);
        }
    }

    let flat = unresolved_flat_bindings();

    if missing.is_empty() && flat.is_empty() {
        println!(
            "all {} documented keys resolve, and every one of the {} non-modal bindings in the \
             command table",
            normal.len() + 5,
            commands::COMMANDS
                .iter()
                .filter(|command| !command.flat.is_empty())
                .count()
        );
    } else {
        if !missing.is_empty() {
            println!("UNRESOLVED: {missing:?}");
        }
        if !flat.is_empty() {
            println!("UNRESOLVED (non-modal, from the command table): {flat:?}");
        }
        std::process::exit(1);
    }
}

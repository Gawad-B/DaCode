//! Every key the cheat sheet claims, fed through the real grammar.
use vex_input::{Grammar, Key, Resolution};
use vex_theme::Mode;

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

    if missing.is_empty() {
        println!("all {} documented keys resolve", normal.len() + 5);
    } else {
        println!("UNRESOLVED: {missing:?}");
        std::process::exit(1);
    }
}

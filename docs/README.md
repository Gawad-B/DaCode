# DaCode documentation

Four kinds of document, because they answer four different questions and mixing them serves none of
them well. This split follows [Diátaxis](https://diataxis.fr/): if you cannot find something, the
question you are asking tells you which column to look in.

| | You are **working** | You are **studying** |
|---|---|---|
| **Practical steps** | [Guides](#guides) — how do I do this specific thing? | [Getting started](getting-started.md) — I am new, teach me |
| **Information** | [Reference](#reference) — what exactly does this do? | [Architecture](explanation/architecture.md) — why is it like this? |

---

## Getting started

**[getting-started.md](getting-started.md)** — installing, the three first-run questions, opening a
project, and editing and saving your first file. Start here if you have not run DaCode before. It
assumes nothing and takes about ten minutes.

## Guides

Goal-first. Each one takes you from "I want *X*" to having *X*.

| | |
|---|---|
| [Choosing how you type](guides/keymaps.md) | Vim's grammar or conventional keys, what changes, and how to rebind |
| [Source control](guides/source-control.md) | staging, committing, diffs, branches, pull and push — VSCode's Source Control view |
| [Language servers](guides/language-servers.md) | diagnostics, completion and go-to-definition — including installing a server from inside the editor |
| [Viewing files](guides/viewing-files.md) | images, PDFs, and the text of office documents, in a tab |
| [Configuration](guides/configuration.md) | the settings tab, `init.lua`, and which one wins |
| [Themes](guides/themes.md) | the five built in, and importing one you found |
| [Migrating from VSCode](guides/migrating-from-vscode.md) | what comes across, what does not, and why |

## Reference

Consult while working. Exhaustive, and derived from the source rather than from memory.

| | |
|---|---|
| [Settings](reference/settings.md) | every option, its type, its default, and where it is read |
| [Command line](reference/cli.md) | every flag, every environment variable, and `dacode import` |
| [Files on disk](reference/files.md) | where configuration, sessions and crash recovery live, per platform |
| [Keys](../CHEATSHEET.md) | every binding in both grammars — checked against the real grammar by a test |

## Explanation

Background and reasoning. Read when you want to understand rather than to act.

| | |
|---|---|
| [Architecture](explanation/architecture.md) | the thirteen crates, the one rule, and how the design follows from it |
| [PLAN.md](../PLAN.md) | the full decision record, including the decisions that turned out wrong |
| [CLAUDE.md](../CLAUDE.md) | working notes for changing the code: the invariants and what breaking each one looked like |

---

## Also here

- **[CHANGELOG.md](../CHANGELOG.md)** — what changed in each release, and whether it is worth upgrading for.
- **[CONTRIBUTING.md](../CONTRIBUTING.md)** — building, testing, and the conventions a change has to hold.
- **[SECURITY.md](../SECURITY.md)** — what to do if you find something, and what is in scope.

Inside the editor, <kbd>F1</kbd> lists every key that works right now and
<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>P</kbd> searches every command — both read the same table the
documentation does, so neither can drift from what the keys actually do.

Something here wrong or missing? [Open an issue](https://github.com/Gawad-B/DaCode/issues/new/choose).

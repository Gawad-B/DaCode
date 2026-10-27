# Security

## Supported versions

DaCode is young and has one maintainer, so there is one supported line: the most recent release. Fixes
land on `main` and go out in the next release; nothing is backported.

| Version | Supported |
|---|---|
| 0.4.x | Yes |
| Earlier | No — upgrade |

## Reporting a vulnerability

Use GitHub's private vulnerability reporting:

**https://github.com/Gawad-B/DaCode/security/advisories/new**

There is no security mailing address, and that link is the only private channel. Please do not open a
public issue for something exploitable, and please do not post a proof of concept anywhere public
before there is a fix.

Include, as far as you have it: what you did, what happened, which version and platform, and the
input that reproduces it — a file, a `.vsix`, an `init.lua`, a theme, a captured language-server
reply. The input matters more than the prose here, because most of what follows is about DaCode reading
something it did not write.

## What to expect

One person, not a company, so: an acknowledgement within about a week, then an honest answer about
whether it is in scope and what it would take to fix. If it is real, a fix on `main` and a release,
and a GitHub Security Advisory crediting you unless you would rather it did not. There is no bounty
programme and no legal safe-harbour boilerplate — this is a text editor written in the open.

## Scope

What follows is what DaCode actually does with input it did not write. It is deliberately concrete: a
report that lands in one of these is worth a lot more than one that names a class of bug.

**It opens files it did not write.** `crates/dacode/src/open.rs` is the only door, and every refusal
there is a sentence for the screen. Reading is bounded by `open::MAX_BYTES`, and binary files and
non-UTF-8 files are refused rather than mangled. A file that makes DaCode panic, corrupt a *different*
file, read or write outside itself, or hang unrecoverably is in scope. Note the documented
limitations before reporting slowness as a vulnerability: opening a file is still synchronous, and
buffer search is still linear in the document. Both are stated in the README and in the doc comments,
and neither is a security finding on its own.

**It runs a Lua program from your config directory.** `init.lua` is code — it is executed through
`mlua` at startup, on purpose, because a keymap is a list of statements rather than a table of
strings. Config is trusted: anyone who can write your config directory has already won, and that is
not a vulnerability. What *is* in scope is a route by which something less trusted becomes Lua that
runs — an imported VSCode setting, a theme file, a project-local file, or anything else DaCode reads
from a repository you merely opened.

**It spawns language servers and formatters found on `PATH`.** `dacode_lsp::resolve` deliberately
returns the resolved `PathBuf` rather than a yes/no, and that resolved path is what goes to
`Command::new`, so the standard library routes a `.cmd` through `cmd.exe` with the argument escaping
added for CVE-2024-24576. Hand-rolling `cmd /c` anywhere would reopen that hole; if you find one, it
is in scope. A configured command containing a path separator is treated as a path and resolved
against the *project root*, so a project that can influence configuration into pointing at a program
inside itself is worth reporting. `crates/dacode/src/install.rs` will run a language-server install
command, but only the exact command it printed, only after being asked twice, never through a shell
and never as root — anything that makes it run something else is in scope.

**It applies edits a language server sends back.** `crates/dacode/src/edits.rs` applies workspace edits,
including cross-file renames. A language server is roughly a program you chose to run, so a hostile
one is not by itself a finding; a server-supplied edit that writes *outside the project root* is.

**It imports VSCode data and `.vsix` archives.** A `.vsix` is a zip.
`crates/dacode-import/src/vsix.rs` reads entries into memory instead of extracting them to disk, keeps
only JSON, and skips any entry over 4 MB — so a zip-slip write, a decompression bomb, or a crafted
manifest that reaches the filesystem is in scope. `dacode import --openvsx` downloads from Open VSX over
HTTPS by shelling out to `curl` or `wget` rather than linking a TLS stack; a way to make that fetch
or write somewhere other than the requested archive is in scope.

**Extension JavaScript is never executed, by design.** DaCode has no JS runtime — extension *data*
(themes, snippets, keybindings, language configuration) imports, and anything whose value is its own
JavaScript does not. §5a of `PLAN.md` has the full account. Any path by which imported extension
content ends up executing is a serious finding.

**Path containment is decided lexically.** `dacode_project::within` folds `.` and `..` itself and
refuses a climb above the root, because `canonicalize` cannot answer for a file that does not exist
yet — which is the normal case for one being created. It fails with `NotFound`, and
`Path::starts_with` compares components and never normalises `..`, so `x/../../../../evil.txt` once
passed the check and `create_dir_all` walked it happily. **A bypass of `within`, or an untrusted path
reaching a write without going through it, is in scope.** It takes the root as a parameter and has
tests; if you can, send a failing test with the report.

## Out of scope

- A crash with no security consequence. Open an ordinary bug report — it is still worth having.
- Anything that first requires write access to the config or state directory. That is already game
  over, and `init.lua` being code is the design.
- Vulnerabilities in `rust-analyzer`, a formatter, `git`, `curl`, `wget`, or any other program DaCode
  merely spawns. Report those upstream; tell us too if DaCode's use of them makes the impact worse.
- Resource exhaustion within documented bounds — a very large file being slow to open, a rare query
  walking a very large buffer. Both limitations are written down rather than hidden.
- Missing compiler or linker hardening flags with no demonstrated impact.

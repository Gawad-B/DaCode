## What changed, and why

Prose, not a list of files — git already knows which files changed. Say what the change does, why it
is the right shape, and anything you tried first and rejected. That last part is what makes the
history worth reading later.

Closes #

## Checks

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean — zero warnings, not fewer
- [ ] `cargo fmt --all` has been run
- [ ] New tests are named as sentences about behaviour, and say *why* where it is not obvious
- [ ] If it adds a guard, I broke the thing it guards and watched it fail

## If it applies

- [ ] **Visual change** — a `--screenshot` render is attached, at a small `--size` too if it involves
      a panel
- [ ] **New command** — all four places: the `Action` variant, `Action::from_name`, `Action::NAMES`,
      and a row in `dacode_input::commands::COMMANDS`
- [ ] **New config option** — all three places: the Lua reader, the settings struct, the settings
      panel
- [ ] **New key binding** — `cargo run -p dacode-input --example cheatsheet` still passes, and
      `CHEATSHEET.md` is updated
- [ ] **Background work** — it carries a `Version`, its `poll()` never blocks, and nothing unbounded
      landed on the UI thread

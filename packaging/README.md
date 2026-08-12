# Packaging

## Linux

```sh
cargo build --release
packaging/linux/install.sh            # into ~/.local
packaging/linux/install.sh /usr/local # or system-wide
```

That places the binary, `vex.desktop` and the icon theme under the chosen prefix. The icon is the
**paper** (light) variant of the mark at every size the handoff ships, plus the scalable SVG —
desktop environments composite app icons onto dark panels far more often than light ones.

The window icon is separate: it is embedded in the binary (`crates/vex/src/icon.rs`), so a vex run
straight out of a build directory has the same icon as an installed one.

## macOS and Windows

Not yet. `logo/design_handoff_vex/logo/README.md` records how to build `.icns` and `.ico` from the
same PNG set when those builds exist.

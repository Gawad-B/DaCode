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

## Packages

```sh
cargo build --release
packaging/build-deb.sh     # target/packages/vex_<version>_amd64.deb
packaging/build-rpm.sh     # needs rpm-build: sudo dnf install rpm-build
```

The `.deb` is assembled with `ar` and `tar` rather than `dpkg-deb`, so it builds on Fedora, on Arch and
in a container with no Debian tooling — which is where release builds usually happen.

Install them the usual way:

```sh
sudo apt install ./target/packages/vex_0.1.0_amd64.deb
sudo dnf install ./target/packages/vex-0.1.0-1.x86_64.rpm
```

## Windows

See `packaging/windows/README.md`. Short version: `cargo build --release`, then
`powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1`.

## macOS

Not yet. `logo/design_handoff_vex/logo/README.md` records how to build `.icns` from the same PNG set
when that build exists.

## CI

`.github/workflows/build.yml` builds all three artifacts on every push — including `vex.exe`, which is
the only way a Windows build gets verified from a Linux machine.

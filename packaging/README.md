# Packaging

## Linux

```sh
cargo build --release
packaging/linux/install.sh            # into ~/.local
packaging/linux/install.sh /usr/local # or system-wide
packaging/linux/uninstall.sh          # same prefix argument, and always use it
```

That places the binary, `dacode.desktop` and the icon theme under the chosen prefix, and `uninstall.sh`
takes them away again. Removing them by hand is what leaves the icon cache claiming a `dacode` icon at
a path that no longer exists — which GTK believes, and which shadows the icon a later `.rpm` or
`.deb` installs into `/usr`, because `~/.local` is searched first. That is a blank tile in the
launcher with a perfectly good icon installed one directory down. The icon is the
**paper** (light) variant of the mark at every size the handoff ships, plus the scalable SVG —
desktop environments composite app icons onto dark panels far more often than light ones.

The window icon is separate: it is embedded in the binary (`crates/dacode/src/icon.rs`), so a DaCode run
straight out of a build directory has the same icon as an installed one.

## Packages

```sh
cargo build --release
packaging/build-deb.sh     # target/packages/dacode_<version>_amd64.deb
packaging/build-rpm.sh     # needs rpm-build: sudo dnf install rpm-build
```

The `.deb` is assembled with `ar` and `tar` rather than `dpkg-deb`, so it builds on Fedora, on Arch and
in a container with no Debian tooling — which is where release builds usually happen.

Install them the usual way:

```sh
sudo apt install ./target/packages/dacode_0.1.0_amd64.deb
sudo dnf install ./target/packages/dacode-0.1.0-1.x86_64.rpm
```

## Windows

See `packaging/windows/README.md`. Short version: `cargo build --release`, then
`powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1`.

## macOS

On a Mac, with Xcode's command-line tools:

```sh
packaging/macos/build-app.sh
```

It builds `aarch64-apple-darwin` and `x86_64-apple-darwin`, joins them with `lipo` into one universal
binary, makes `DaCode.app` (`packaging/macos/Info.plist`, an `.icns` from the Linux PNG set), signs it
ad hoc, and writes two files to `target/packages`:

- `dacode-<version>-macos.dmg` — what a person downloads: the app beside an Applications link.
- `dacode-<version>-macos.zip` — what the updater downloads (`update::mac_zip`), made with `ditto`
  so the bundle's permissions and signature survive.

There is no Developer ID signing or notarization: Gatekeeper asks once on first launch. Adding them
means a `codesign --sign "Developer ID Application: …" --options runtime` in place of the ad-hoc
one, then `xcrun notarytool submit --wait` and `xcrun stapler staple` on the `.dmg`, with the
certificate and an app-specific password as GitHub secrets.

## CI

`.github/workflows/build.yml` builds every artifact on every push — including `dacode.exe` and the
macOS `.dmg`, which is the only way either build gets verified from a Linux machine.

The Mac-only code can at least be *compiled* from Linux: `packaging/macos/check-from-linux.sh`
type-checks the editor for `aarch64-apple-darwin`, with stand-in C tools for the build scripts. It
catches a Mac-only type error before CI does, and says nothing about how the code runs.

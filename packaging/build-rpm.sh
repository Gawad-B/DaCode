#!/bin/sh
# Builds vex-<version>-1.x86_64.rpm from the release binary.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$here/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/vex/Cargo.toml" | head -1)"
out="${1:-$root/target/packages}"

command -v rpmbuild >/dev/null || {
    echo "rpmbuild is not installed. On Fedora: sudo dnf install rpm-build" >&2
    exit 1
}
[ -x "$root/target/release/vex" ] || { echo "build it first: cargo build --release" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/SOURCES" "$work/BUILD" "$work/RPMS"

cp "$root/target/release/vex" "$work/SOURCES/"
cp "$here/linux/vex.desktop" "$work/SOURCES/"
cp -r "$here/linux/icons" "$work/SOURCES/"
cp "$root/README.md" "$root/CHEATSHEET.md" "$root/CHANGELOG.md" "$work/SOURCES/"
cp "$root/examples/init.lua" "$work/SOURCES/"
# The README's links into docs/ are relative, so the tree ships with it.
cp -r "$root/docs" "$work/SOURCES/"

rpmbuild -bb "$here/vex.spec" \
    --define "_topdir $work" \
    --define "vex_version $version" \
    --quiet

mkdir -p "$out"
find "$work/RPMS" -name '*.rpm' -exec cp {} "$out/" \;
find "$out" -name "vex-$version*.rpm"

#!/bin/sh
# Builds DaCode.app for macOS — one universal binary for Apple Silicon and Intel — and packs it
# twice: a .dmg for a person to drag into Applications, and a .zip for the editor's own updater.
#
# Runs on a Mac only: `lipo`, `iconutil`, `sips`, `codesign`, `ditto` and `hdiutil` are Apple's.
#
#     packaging/macos/build-app.sh            # builds both architectures, then packs
#     packaging/macos/build-app.sh --skip-build
#
# The bundle is signed ad hoc (`codesign -s -`), not with a Developer ID: there is no Apple
# Developer account behind this project yet. Apple Silicon refuses to run an unsigned arm64 binary
# at all, and `lipo` invalidates the linker's own ad-hoc signature, so this step is what makes the
# binary runnable — it is not what makes Gatekeeper trust it. A downloaded copy still asks once
# (right-click → Open, or System Settings → Privacy & Security → Open Anyway); the README says so.
#
# The zip's name is what `dacode::update::mac_zip` looks for, and DaCode.app inside it is what
# `unpack_mac` expects. Change one and the updater stops finding the other.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/dacode/Cargo.toml" | head -1)"
out="$root/target/packages"
targets="aarch64-apple-darwin x86_64-apple-darwin"

if [ "${1:-}" != "--skip-build" ]; then
    for target in $targets; do
        rustup target add "$target" >/dev/null
        # 11.0 is the oldest macOS the Info.plist admits; without this the binary would require
        # whatever the build machine runs.
        MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release -p dacode --target "$target" \
            --manifest-path "$root/Cargo.toml"
    done
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/DaCode.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$out"

lipo -create -output "$app/Contents/MacOS/dacode" \
    "$root/target/aarch64-apple-darwin/release/dacode" \
    "$root/target/x86_64-apple-darwin/release/dacode"
lipo -info "$app/Contents/MacOS/dacode"

sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

# The icon from the same PNG set the Linux packages install. There is no 1024px source, so the
# 512@2x slot is the 512 scaled up — slightly soft at that one size, which only the Finder's
# largest preview shows.
icons="$root/packaging/linux/icons/hicolor"
set_dir="$work/dacode.iconset"
mkdir -p "$set_dir"
for size in 16 32 128 256 512; do
    cp "$icons/${size}x${size}/apps/dacode.png" "$set_dir/icon_${size}x${size}.png"
done
cp "$icons/32x32/apps/dacode.png" "$set_dir/icon_16x16@2x.png"
cp "$icons/64x64/apps/dacode.png" "$set_dir/icon_32x32@2x.png"
cp "$icons/256x256/apps/dacode.png" "$set_dir/icon_128x128@2x.png"
cp "$icons/512x512/apps/dacode.png" "$set_dir/icon_256x256@2x.png"
sips -z 1024 1024 "$icons/512x512/apps/dacode.png" --out "$set_dir/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/dacode.icns"

cp "$root/LICENSE" "$app/Contents/Resources/LICENSE" 2>/dev/null || true

codesign --force --deep --sign - "$app"
codesign --verify --verbose "$app"

zip="$out/dacode-$version-macos.zip"
rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"

# The disk image: the app beside a link to /Applications, which is the whole of a Mac install.
dmg="$out/dacode-$version-macos.dmg"
image="$work/image"
mkdir -p "$image"
ditto "$app" "$image/DaCode.app"
ln -s /Applications "$image/Applications"
rm -f "$dmg"
hdiutil create -volname "DaCode $version" -srcfolder "$image" -ov -format UDZO "$dmg"

ls -l "$zip" "$dmg"

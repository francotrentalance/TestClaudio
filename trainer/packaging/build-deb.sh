#!/bin/bash
# Packages a release binary as a .deb for Linux Mint / Ubuntu / Debian (amd64).
#
#   packaging/build-deb.sh <path/to/holdem-trainer> <output-dir>
#
# Needs dpkg-deb and objdump. The binary should be built for a baseline CPU
# (RUSTFLAGS="-C target-cpu=x86-64-v2") on the oldest supported distribution
# (Mint 21 / Ubuntu 22.04, glibc 2.35).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
bin=$1
out=$2
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
pkg=$work/holdem-trainer

install -Dm755 "$bin" "$pkg/usr/bin/holdem-trainer"
install -Dm644 "$here/holdem-trainer.desktop" "$pkg/usr/share/applications/holdem-trainer.desktop"
for s in 16 32 48 64 128 256 512; do
    install -Dm644 "$root/assets/icon-$s.png" "$pkg/usr/share/icons/hicolor/${s}x${s}/apps/holdem-trainer.png"
done
install -Dm644 "$root/assets/icon.svg" "$pkg/usr/share/icons/hicolor/scalable/apps/holdem-trainer.svg"
install -Dm644 "$here/copyright" "$pkg/usr/share/doc/holdem-trainer/copyright"
install -Dm644 "$root/assets/fonts/OFL.txt" "$pkg/usr/share/doc/holdem-trainer/OFL.txt"

# Oldest glibc symbol version the binary needs.
glibc=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | cut -d_ -f2 | sort -Vu | tail -n1)
size=$(du -sk "$pkg" | cut -f1)

# The window system libraries are loaded at run time (dlopen), so they are
# listed by hand: X11 and Wayland, OpenGL through libglvnd, xkbcommon for keys.
mkdir -p "$pkg/DEBIAN"
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: holdem-trainer
Version: $version
Architecture: amd64
Maintainer: Hold'em Trainer <francotrentalance@users.noreply.github.com>
Installed-Size: $size
Depends: libc6 (>= $glibc), libgcc-s1, libgl1, libegl1, libx11-6, libx11-xcb1, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1
Section: games
Priority: optional
Homepage: https://github.com/francotrentalance/TestClaudio
Description: GTO quiz trainer for 6-max No-Limit Hold'em
 Hold'em Trainer deals you hands and grades every decision against
 solver strategies: 6-max cash preflop at 100bb, tournament push/fold
 from 5 to 20bb, and postflop BTN vs BB pots where the turn and river
 are solved live for the board that comes.
 .
 Earn XP, level up, keep a streak and a daily goal, and review your
 mistakes with the full solution.
CONTROL

mkdir -p "$out"
deb="$out/holdem-trainer_${version}_amd64.deb"
dpkg-deb --build --root-owner-group -Zxz "$pkg" "$deb" >/dev/null
echo "$deb"

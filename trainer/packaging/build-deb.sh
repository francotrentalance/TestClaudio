#!/bin/bash
# Builds the release binary and packages it as a .deb for Linux Mint / Ubuntu /
# Debian (amd64).
#
#   packaging/build-deb.sh <output-dir>
#
# Needs Rust 1.89 or newer (rustup; Mint's apt cargo is too old), a C compiler,
# python3, dpkg-deb and objdump. Run it on the oldest supported
# distribution (Mint 21 / Ubuntu 22.04, glibc 2.35): the binary needs that glibc
# or newer. It is built for the baseline x86-64 CPU, because Mint runs on CPUs
# without SSE4.2/POPCNT. RUSTFLAGS overrides the repo's target-cpu=native
# (.cargo/config.toml), and target-deb/ keeps that from rebuilding the native
# dev build. Set DEB_REVISION to add a Debian revision (0.2.0-2) for a rebuild
# of the same version.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
out=${1:?usage: packaging/build-deb.sh <output-dir>}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)${DEB_REVISION:+-$DEB_REVISION}

export CARGO_TARGET_DIR="$root/target-deb"
env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="-C target-cpu=x86-64" cargo build --release --locked --manifest-path "$root/Cargo.toml" --bin holdem-trainer
bin="$CARGO_TARGET_DIR/release/holdem-trainer"

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
pkg=$work/holdem-trainer
doc=$pkg/usr/share/doc/holdem-trainer

install -Dm755 "$bin" "$pkg/usr/bin/holdem-trainer"
install -Dm644 "$here/holdem-trainer.desktop" "$pkg/usr/share/applications/holdem-trainer.desktop"
for s in 16 32 48 64 128 256 512; do
    install -Dm644 "$root/assets/icon-$s.png" "$pkg/usr/share/icons/hicolor/${s}x${s}/apps/holdem-trainer.png"
done
install -Dm644 "$root/assets/icon.svg" "$pkg/usr/share/icons/hicolor/scalable/apps/holdem-trainer.svg"
install -Dm644 "$here/copyright" "$doc/copyright"
install -Dm644 "$root/assets/fonts/OFL.txt" "$doc/OFL.txt"
# Licences of egui's default fonts, which are embedded too.
for f in UFL.txt OFL-NotoEmoji.txt Hack-Regular.txt emoji-icon-font-mit-license.txt; do
    install -Dm644 "$root/assets/fonts/egui/$f" "$doc/$f"
done
# The crates in the binary, fresh from Cargo.lock.
python3 "$here/third-party-notices.py" "$doc/THIRD-PARTY-NOTICES"
chmod 644 "$doc/THIRD-PARTY-NOTICES"
cmp -s "$doc/THIRD-PARTY-NOTICES" "$here/THIRD-PARTY-NOTICES" || echo "warning: packaging/THIRD-PARTY-NOTICES is out of date; run packaging/third-party-notices.py" >&2

# Oldest glibc symbol version the binary needs.
glibc=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | cut -d_ -f2 | sort -Vu | tail -n1)
size=$(du -sk "$pkg" | cut -f1)

# The window system libraries are loaded at run time (dlopen), so they are
# listed by hand: X11 and Wayland, OpenGL through libglvnd, xkbcommon for keys.
# zenity shows startup errors and crashes, which would otherwise go unseen.
mkdir -p "$pkg/DEBIAN"
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: holdem-trainer
Version: $version
Architecture: amd64
Maintainer: Hold'em Trainer <francotrentalance@users.noreply.github.com>
Installed-Size: $size
Depends: libc6 (>= $glibc), libgcc-s1, libgl1, libegl1, libx11-6, libx11-xcb1, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1
Recommends: zenity
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

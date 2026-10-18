#!/usr/bin/env bash
# Build and install holdem-solver on Linux Mint (21.x, 22.x, LMDE), Ubuntu or Debian.
#
#   ./install.sh               build with the system Rust if it is new enough, else rustup
#   ./install.sh --rustup      always use rustup's latest stable Rust (fastest binary)
#   ./install.sh --prefix DIR  install the binary into DIR (default: ~/.local/bin)
#   ./install.sh --no-install  only build (binary at target/release/holdem-solver)
set -euo pipefail

MIN_RUST="1.75.0"
PREFIX="$HOME/.local/bin"
USE_RUSTUP=0
INSTALL=1

while [ $# -gt 0 ]; do
    case "$1" in
        --rustup) USE_RUSTUP=1 ;;
        --prefix) PREFIX="$2"; shift ;;
        --no-install) INSTALL=0 ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

cd "$(dirname "$0")"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Run a command as root (directly when already root, e.g. in a container).
as_root() {
    if [ "$(id -u)" -eq 0 ]; then "$@"
    elif command -v sudo >/dev/null; then sudo "$@"
    else die "need root to run: $*"
    fi
}

# version_ge A B: true when version A >= version B.
version_ge() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]; }

cargo_version() { cargo --version 2>/dev/null | awk '{print $2}' | sed 's/-.*//'; }

apt_install() {
    command -v apt-get >/dev/null || die "apt-get not found; install $* with your package manager"
    say "Installing $* with apt (may ask for your password)"
    as_root apt-get update -qq
    as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq -o Dpkg::Use-Pty=0 "$@" >/dev/null
}

# The solver links a small C allocator (mimalloc), and Rust needs a linker.
if ! command -v cc >/dev/null; then
    apt_install build-essential
fi

# Pick a Rust toolchain.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if [ "$USE_RUSTUP" -eq 1 ] || ! command -v cargo >/dev/null || ! version_ge "$(cargo_version)" "$MIN_RUST"; then
    have="$(cargo_version || true)"
    apt_candidate=""
    if [ "$USE_RUSTUP" -eq 0 ] && command -v apt-cache >/dev/null; then
        apt_candidate="$(apt-cache policy cargo 2>/dev/null | awk '/Candidate:/ {print $2}' | sed 's/+.*//; s/~.*//')"
    fi
    if [ -n "$apt_candidate" ] && [ "$apt_candidate" != "(none)" ] && version_ge "$apt_candidate" "$MIN_RUST"; then
        apt_install cargo rustc
    else
        [ -n "$have" ] && say "cargo $have is older than $MIN_RUST"
        say "Installing Rust with rustup (https://rustup.rs) into ~/.cargo"
        # Mint ships wget but not curl; rustup's bootstrap script works with either.
        if command -v curl >/dev/null; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        elif command -v wget >/dev/null; then
            wget --https-only -qO- https://sh.rustup.rs | sh -s -- -y --profile minimal
        else
            apt_install curl ca-certificates
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        fi
        . "$HOME/.cargo/env"
        if [ "$USE_RUSTUP" -eq 1 ]; then
            rustup update stable --no-self-update >/dev/null
            rustup default stable >/dev/null
        fi
    fi
fi

version_ge "$(cargo_version)" "$MIN_RUST" || die "cargo $(cargo_version) is too old (need $MIN_RUST)"
say "Building with $(rustc --version)"
cargo build --release

BIN="target/release/holdem-solver"
"$BIN" --version >/dev/null || die "the built binary does not run"

if [ "$INSTALL" -eq 1 ]; then
    mkdir -p "$PREFIX"
    install -m 755 "$BIN" "$PREFIX/holdem-solver"
    say "Installed $PREFIX/holdem-solver"
    case ":$PATH:" in
        *":$PREFIX:"*) ;;
        *)
            # Mint only adds ~/.local/bin to PATH at login (~/.profile), so new
            # terminals would not find the command until the next login.
            line="export PATH=\"$PREFIX:\$PATH\"  # added by holdem-solver install.sh"
            if [ -f "$HOME/.bashrc" ] && ! grep -qF "added by holdem-solver install.sh" "$HOME/.bashrc"; then
                printf '\n%s\n' "$line" >> "$HOME/.bashrc"
                say "Added $PREFIX to PATH in ~/.bashrc (new terminals will find holdem-solver)"
            fi
            echo "For this terminal, run:  export PATH=\"$PREFIX:\$PATH\""
            ;;
    esac
else
    say "Built $BIN"
fi
echo "Try:  holdem-solver preflop --help"

#!/usr/bin/env bash
# Build and install holdem-solver on Linux Mint (21.x, 22.x, LMDE), Ubuntu or Debian.
#
#   ./install.sh               build with the system Rust if it is new enough, else rustup
#   ./install.sh --rustup      always use rustup's latest stable Rust (fastest binary)
#   ./install.sh --prefix DIR  install the binary into DIR (default: ~/.local/bin)
#   ./install.sh --no-install  only build (binary at target/release/holdem-solver)
#
# Run it as your normal user, not with sudo: it asks for your password only
# when it needs to install packages with apt.

# `sh install.sh` runs dash, which lacks bash features used below: re-run under bash.
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi
set -euo pipefail

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

MIN_RUST="1.75.0"
PREFIX="$HOME/.local/bin"
USE_RUSTUP=0
INSTALL=1

while [ $# -gt 0 ]; do
    case "$1" in
        --rustup) USE_RUSTUP=1 ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift ;;
        --prefix=*) PREFIX="${1#--prefix=}" ;;
        --no-install) INSTALL=0 ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
        *) die "unknown option: $1 (see ./install.sh --help)" ;;
    esac
    shift
done
[ -n "$PREFIX" ] || die "--prefix needs a directory"
# bash does not expand ~ inside --prefix=~/bin.
case "$PREFIX" in
    "~") PREFIX="$HOME" ;;
    "~/"*) PREFIX="$HOME/${PREFIX#"~/"}" ;;
esac
# Resolve before changing directory, so a relative --prefix means what the user typed.
PREFIX="$(realpath -m -- "$PREFIX")"

# Under sudo, HOME is /root: the binary would land in /root/.local/bin and the
# build directory would become root-owned, breaking later normal runs.
if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then
    die "don't run this with sudo. Run it as $SUDO_USER:  ./install.sh  (it asks for your password only when it installs packages)"
fi

cd "$(dirname "$0")"

if [ -e target ] && ! [ -w target ]; then
    die "target/ is not writable (probably left by an earlier 'sudo ./install.sh'). Fix it with:  sudo chown -R $(id -un): target"
fi

# Run a command as root: directly when already root (e.g. in a container),
# otherwise through sudo. Returns non-zero instead of exiting so callers can fall back.
as_root() {
    if [ "$(id -u)" -eq 0 ]; then "$@"
    elif command -v sudo >/dev/null; then sudo "$@"
    else return 1
    fi
}

# Whether this account can install packages (root, or allowed to use sudo).
# Checked up front so a standard account is not asked for a password that cannot work.
can_admin() {
    [ "$(id -u)" -eq 0 ] && return 0
    command -v sudo >/dev/null || return 1
    sudo -n true 2>/dev/null && return 0
    id -nG | tr ' ' '\n' | grep -qxE 'sudo|admin|wheel'
}

# version_ge A B: true when version A >= version B.
version_ge() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]; }

cargo_version() { cargo --version 2>/dev/null | awk '{print $2}' | sed 's/-.*//'; }

# Installable version of an apt package, or empty. LC_ALL=C because apt-cache
# translates its labels ('Candidato:' on an Italian desktop).
apt_candidate() {
    command -v apt-cache >/dev/null || return 0
    local v
    v="$(LC_ALL=C apt-cache policy "$1" 2>/dev/null | awk '/Candidate:/ {print $2}' | sed 's/^[0-9]*://; s/[+~].*//')"
    case "$v" in [0-9]*) echo "$v" ;; esac
}

# apt-get update, waiting for Update Manager and friends to release the lists
# lock. Other errors (typically a broken third-party repository, e.g. one whose
# signing key expired) are reported but do not stop the install.
apt_update() {
    local log tries=0
    log="$(mktemp)"
    while true; do
        if as_root apt-get update -q 2>"$log" >/dev/null; then
            rm -f "$log"
            return 0
        fi
        if grep -qE "Could not get lock|Unable to lock" "$log" && [ "$tries" -lt 60 ]; then
            [ "$tries" -eq 0 ] && say "Another program (Update Manager?) is using apt; waiting for it to finish..."
            tries=$((tries + 1))
            sleep 10
            continue
        fi
        cat "$log" >&2
        rm -f "$log"
        say "apt-get update reported errors (usually from an unrelated repository); continuing"
        APT_UPDATE_FAILED=1
        return 0
    done
}

apt_install() {
    command -v apt-get >/dev/null || return 1
    if ! can_admin; then
        say "This account cannot use sudo, so apt cannot install $*"
        return 1
    fi
    say "Installing $* with apt (may ask for your password; the download can take a few minutes)"
    local opts=(-y -q --no-remove -o DPkg::Lock::Timeout=600)
    # The current package lists usually already have what we need, so skip
    # 'apt-get update' unless the install fails (e.g. lists too old).
    if as_root env DEBIAN_FRONTEND=noninteractive apt-get install "${opts[@]}" "$@"; then
        return 0
    fi
    say "Refreshing the package lists and trying again"
    apt_update
    if as_root env DEBIAN_FRONTEND=noninteractive apt-get install "${opts[@]}" "$@"; then
        return 0
    fi
    if [ "${APT_UPDATE_FAILED:-0}" -eq 1 ]; then
        echo "Note: apt-get update also failed. If the messages above name a repository ('not signed'," >&2
        echo "'NO_PUBKEY'), fix it on Linux Mint in Software Sources > Maintenance > 'Add missing keys'," >&2
        echo "or disable that repository there." >&2
    fi
    return 1
}

# Download and run the rustup installer (into ~/.cargo, no admin rights needed).
bootstrap_rustup() {
    say "Installing Rust with rustup (https://rustup.rs) into ~/.cargo"
    local url="https://sh.rustup.rs" tmp
    tmp="$(mktemp)"
    # Mint ships wget but not curl; rustup's installer works with either.
    if ! command -v curl >/dev/null && ! command -v wget >/dev/null; then
        apt_install curl ca-certificates || die "need curl or wget to download rustup"
    fi
    if command -v curl >/dev/null; then
        curl --proto '=https' --tlsv1.2 -fsS --connect-timeout 30 -o "$tmp" "$url" ||
            die "could not download $url (check your internet connection)"
    else
        wget --https-only -nv --timeout=30 --tries=3 -O "$tmp" "$url" ||
            die "could not download $url (check your internet connection)"
    fi
    sh "$tmp" -y --profile minimal || die "the rustup installer failed (see above)"
    rm -f "$tmp"
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
}

# Use rustup's current stable for this build without changing the user's default toolchain.
use_rustup_stable() {
    command -v rustup >/dev/null || bootstrap_rustup
    say "Using rustup's stable Rust for this build"
    rustup toolchain install stable --profile minimal --no-self-update
    export RUSTUP_TOOLCHAIN=stable
}

# The solver links a small C allocator (mimalloc), and Rust needs a linker.
if ! command -v cc >/dev/null; then
    apt_install build-essential || die "a C compiler is required. Install it with:  sudo apt install build-essential"
fi

# Pick a Rust toolchain.
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if [ "$USE_RUSTUP" -eq 1 ]; then
    use_rustup_stable
elif command -v cargo >/dev/null && version_ge "$(cargo_version)" "$MIN_RUST"; then
    :
elif command -v rustup >/dev/null; then
    # An older rustup toolchain would shadow apt's cargo in PATH, so stay with rustup.
    say "cargo $(cargo_version) is older than $MIN_RUST"
    use_rustup_stable
else
    cand="$(apt_candidate cargo)"
    if [ -n "$cand" ] && version_ge "$cand" "$MIN_RUST" && apt_install cargo rustc; then
        :
    else
        [ -n "$cand" ] && version_ge "$cand" "$MIN_RUST" && say "apt could not install Rust; using rustup instead (no admin rights needed)"
        use_rustup_stable
    fi
fi

version_ge "$(cargo_version)" "$MIN_RUST" ||
    die "$(command -v cargo || echo cargo) is version $(cargo_version), older than $MIN_RUST. Run:  ./install.sh --rustup"

# Build and print the path of the produced binary. --locked keeps the
# dependency versions pinned in Cargo.lock (chosen to build on Rust 1.75), and
# cargo reports where it put the binary, whatever target-dir settings exist.
build() {
    say "Building with $(rustc --version)" >&2
    local out
    out="$(mktemp)"
    if ! cargo build --release --locked --message-format=json-render-diagnostics >"$out"; then
        rm -f "$out"
        return 1
    fi
    grep -o '"executable":"[^"]*holdem-solver"' "$out" | tail -n1 | cut -d'"' -f4
    rm -f "$out"
}

if ! BIN="$(build)"; then
    if [ "${RUSTUP_TOOLCHAIN:-}" = stable ]; then
        die "the build failed (see the messages above)"
    fi
    say "The build failed with $(rustc --version | cut -d' ' -f1-2); retrying with rustup's stable Rust"
    use_rustup_stable
    BIN="$(build)" || die "the build failed (see the messages above)"
fi
[ -n "$BIN" ] && [ -x "$BIN" ] || die "the build finished but cargo did not report the holdem-solver binary"
"$BIN" --version >/dev/null || die "the built binary at $BIN does not run on this machine"

# Add PREFIX to PATH in the startup file of the user's shell. Mint only adds
# ~/.local/bin to PATH at login (~/.profile), so new terminals would otherwise
# not find the command until the next login.
add_to_path() {
    local marker="added by holdem-solver install.sh" shell_name rc line
    shell_name="$(basename "$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)" 2>/dev/null || true)"
    [ -n "$shell_name" ] || shell_name="$(basename "${SHELL:-bash}")"
    case "$shell_name" in
        fish)
            rc="$HOME/.config/fish/conf.d/holdem-solver.fish"
            line="fish_add_path -g \"$PREFIX\"  # $marker"
            mkdir -p "$(dirname "$rc")"
            ;;
        zsh) rc="${ZDOTDIR:-$HOME}/.zshrc"; line="export PATH=\"$PREFIX:\$PATH\"  # $marker" ;;
        *) rc="$HOME/.bashrc"; line="export PATH=\"$PREFIX:\$PATH\"  # $marker" ;;
    esac
    if [ -f "$rc" ] && grep -qF "$marker" "$rc"; then
        return
    fi
    printf '\n%s\n' "$line" >> "$rc"
    say "Added $PREFIX to PATH in $rc (new terminals will find holdem-solver)"
}

if [ "$INSTALL" -eq 1 ]; then
    mkdir -p "$PREFIX"
    install -m 755 "$BIN" "$PREFIX/holdem-solver"
    say "Installed $PREFIX/holdem-solver"
    case ":$PATH:" in
        *":$PREFIX:"*) ;;
        *)
            add_to_path
            echo "For this terminal, run:  export PATH=\"$PREFIX:\$PATH\""
            ;;
    esac
else
    say "Built $BIN"
fi
echo "Try:  holdem-solver preflop --help"

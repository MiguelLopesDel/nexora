#!/usr/bin/env bash
# One-shot local install: system deps (apt/dnf), cargo build --release, and
# copy the binary + desktop entry into the user's XDG dirs. Never re-runs
# itself or checks for updates — see `nexora update` for that (todo), or just
# re-run this script (it updates its own checkout when piped, see below).
#
# Works two ways:
#   - From a clone of this repo: ./scripts/install.sh
#   - Piped straight from GitHub, no clone needed:
#       curl -fsSL https://raw.githubusercontent.com/MiguelLopesDel/nexora/main/scripts/install.sh | bash
#     This clones (or updates, on a later run) main into
#     ~/.local/share/nexora/src and builds from there.
#
# Works on Arch/pacman too. Prefer packaging/aur/PKGBUILD (makepkg -si) or
# the AUR package if you want pacman to track nexora as an installed
# package; this script is the no-AUR-account fallback.
set -euo pipefail

REPO_URL="https://github.com/MiguelLopesDel/nexora.git"
CHECKOUT_DIR="$HOME/.local/share/nexora/src"

script_path="${BASH_SOURCE[0]:-}"
if [[ -n "$script_path" && -f "$script_path" ]]; then
    # Running from a real file: assume it's a checkout of this repo.
    repo_root="$(cd "$(dirname "$script_path")/.." && pwd)"
else
    # Piped in (curl | bash): no local checkout to run from yet.
    if ! command -v git >/dev/null; then
        echo "git not found; install it first (needed to fetch the source)."
        exit 1
    fi
    if [[ -d "$CHECKOUT_DIR/.git" ]]; then
        echo "==> Updating existing checkout in $CHECKOUT_DIR"
        git -C "$CHECKOUT_DIR" pull --ff-only
    else
        echo "==> Cloning nexora into $CHECKOUT_DIR"
        mkdir -p "$(dirname "$CHECKOUT_DIR")"
        git clone "$REPO_URL" "$CHECKOUT_DIR"
    fi
    repo_root="$CHECKOUT_DIR"
fi
cd "$repo_root"

install_deps() {
    if command -v apt-get >/dev/null; then
        echo "==> Installing build/runtime dependencies (apt)"
        sudo apt-get update
        sudo apt-get install -y \
            libgtk-4-dev libgtk4-layer-shell-dev pulseaudio-utils \
            cmake g++ libclang-dev libvulkan-dev glslc
    elif command -v dnf >/dev/null; then
        echo "==> Installing build/runtime dependencies (dnf)"
        sudo dnf install -y \
            gtk4-devel gtk4-layer-shell-devel pulseaudio-utils \
            cmake gcc-c++ clang-devel vulkan-loader-devel glslc
    elif command -v pacman >/dev/null; then
        echo "==> Installing build/runtime dependencies (pacman)"
        echo "    (packaging/aur/PKGBUILD via makepkg -si is the alternative" \
            "if you'd rather have pacman track the install as a package.)"
        sudo pacman -S --needed \
            gtk4 gtk4-layer-shell libpulse cmake gcc clang \
            vulkan-headers vulkan-icd-loader shaderc
    else
        echo "Unrecognized package manager. Install the dependencies listed" \
            "in README.md manually, then re-run with --skip-deps."
        exit 1
    fi
}

if [[ "${1:-}" != "--skip-deps" ]]; then
    install_deps
else
    echo "==> Skipping dependency install (--skip-deps)"
fi

if ! command -v cargo >/dev/null; then
    echo "cargo not found. Install Rust first: https://rustup.rs"
    exit 1
fi

echo "==> Building nexora (release)"
cargo build --release

bin_dir="$HOME/.local/bin"
apps_dir="$HOME/.local/share/applications"
mkdir -p "$bin_dir" "$apps_dir"
install -Dm755 target/release/nexora "$bin_dir/nexora"
install -Dm644 assets/dev.nexora.Nexora.desktop "$apps_dir/dev.nexora.Nexora.desktop"

echo
echo "==> Installed $bin_dir/nexora"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "    $bin_dir is not on your PATH — add it to your shell profile." ;;
esac
echo "==> Next: nexora config init, then bind 'nexora toggle' to a key."

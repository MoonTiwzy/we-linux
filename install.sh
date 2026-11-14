#!/usr/bin/env bash
# One command to a working setup: builds we-linux and the renderer it was made for
# (wepaper), and puts them in ~/.local (PREFIX names another place).
#
# The renderer is the commit flake.lock names: the same one the Nix package is built
# with, so both ways of installing give the same pair. It is built when it is missing
# or is another commit than that one; a pull of we-linux that moves the pin brings
# the renderer along.
#
#   ./install.sh              we-linux, and the renderer when it is not the pinned one
#   ./install.sh --engine     build the renderer again whatever is installed
#   ./install.sh --no-web     leave out web wallpapers (they need GTK 4 and WebKitGTK 6)
#
# WE_LINUX_ENGINE_REV names another commit, branch or tag of the renderer to build.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
engine_repo=${WE_LINUX_ENGINE_REPO:-https://github.com/MoonTiwzy/wepaper}
engine_src="${XDG_CACHE_HOME:-$HOME/.cache}/we-linux/wepaper"
again="" web=1
for arg in "$@"; do
    case $arg in
    --engine) again=1 ;;
    --no-web) web="" ;;
    *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
    esac
done

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
command -v cargo >/dev/null || { echo "cargo is needed: https://rustup.rs" >&2; exit 1; }

say "building we-linux"
cargo build --release --manifest-path "$here/Cargo.toml"
install -Dm755 "$here/target/release/we-linux" "$prefix/bin/we-linux"
install -Dm644 "$here/assets/we-linux.desktop" "$prefix/share/applications/we-linux.desktop"
install -Dm644 "$here/assets/we-linux.svg" "$prefix/share/icons/hicolor/scalable/apps/we-linux.svg"

# The renderer's commit, as flake.lock pins it: the "rev" of the node called wepaper.
pinned=$(awk '/^    "wepaper": \{/ { inside = 1 } inside && /"rev":/ { gsub(/[",]/, "", $2); print $2; exit }' "$here/flake.lock")
wanted=${WE_LINUX_ENGINE_REV:-$pinned}
[[ -n $wanted ]] || { echo "flake.lock does not say which wepaper to build" >&2; exit 1; }
# What is installed says which commit it is in a file beside its plugins.
stamp=$prefix/share/wepaper/revision
have=$(cat "$stamp" 2>/dev/null || true)

if [[ -n $again || ! -x $prefix/bin/wepaper || $have != "$wanted" ]]; then
    say "building wepaper (the renderer) at ${wanted:0:12}: about a minute"
    if [[ ! -d $engine_src/.git ]]; then
        mkdir -p "$(dirname "$engine_src")"
        git init -q "$engine_src"
        git -C "$engine_src" remote add origin "$engine_repo"
    fi
    git -C "$engine_src" fetch -q --depth 1 origin "$wanted"
    git -C "$engine_src" checkout -q --detach FETCH_HEAD
    if ! cargo build --release --manifest-path "$engine_src/Cargo.toml"; then
        cat >&2 <<'HINT'

The renderer did not build. It needs a C compiler and the headers of Wayland:
  Arch:    sudo pacman -S --needed base-devel wayland
  Fedora:  sudo dnf install gcc wayland-devel
  Debian:  sudo apt install build-essential libwayland-dev
HINT
        exit 1
    fi
    install -Dsm755 "$engine_src/target/release/wepaper" "$prefix/bin/wepaper"
    mkdir -p "$prefix/share/wepaper"
    rm -rf "$prefix/share/wepaper/plugins"
    cp -r "$engine_src/plugins" "$prefix/share/wepaper/plugins"
    echo "$wanted" >"$stamp"

    # Web wallpapers are a plugin of their own, built when what it needs is installed.
    if [[ -n $web ]] && pkg-config --exists webkitgtk-6.0 gtk4-layer-shell-0 2>/dev/null; then
        say "building the web wallpaper plugin"
        if cargo build --release --manifest-path "$engine_src/Cargo.toml" -p wepaper-web; then
            install -Dsm755 "$engine_src/target/release/wepaper-web" "$prefix/bin/wepaper-web"
        else
            echo "  the web plugin did not build: web wallpapers will not play, everything else will" >&2
        fi
    elif [[ -n $web ]]; then
        echo "  web wallpapers are left out: install webkitgtk-6.0 and gtk4-layer-shell and run ./install.sh --engine to add them"
    fi
else
    say "wepaper is the pinned one already (${wanted:0:12})"
fi

# Run time: libmpv plays videos and sound, ffmpeg fits videos to the monitor.
for tool in mpv ffmpeg; do
    command -v "$tool" >/dev/null || echo "  $tool is not installed: videos will not play (mpv) or will not be fitted to the monitor (ffmpeg)"
done

say "installed to $prefix/bin: we-linux, wepaper"
case ":$PATH:" in *":$prefix/bin:"*) ;; *) echo "  add $prefix/bin to your PATH" ;; esac
echo "  run: we-linux"
echo "  keep the wallpaper after a login: start 'we-linux daemon' with your session"
echo "  (Hyprland: exec-once = we-linux daemon)"

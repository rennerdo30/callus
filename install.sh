#!/bin/sh
# Callus installer for macOS and Linux (and Git Bash on Windows).
#
#   curl -fsSL https://raw.githubusercontent.com/rennerdo30/callus/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/rennerdo30/callus/main/install.sh | sh -s -- update
#   curl -fsSL https://raw.githubusercontent.com/rennerdo30/callus/main/install.sh | sh -s -- uninstall
#       (arguments: install, update or uninstall, then claude codex opencode antigravity, --yes, --dry-run, --no-packs)
#
# Zero Python or Node.js required: downloads the single prebuilt `callus` release binary
# (or uses $CALLUS_ARCHIVE / $CALLUS_BIN for offline or pre-release testing), installs it into
# ~/.local/share/callus (or $CALLUS_APP) with a symlink in ~/.local/bin/callus, and runs
# `callus install|update|uninstall` which shows exact per-tool steps and asks before each one.

set -eu

main() {
    repo="rennerdo30/callus"
    app_dir="${CALLUS_APP:-${XDG_DATA_HOME:-$HOME/.local/share}/callus}"
    bin_dir="${CALLUS_BIN_DIR:-$HOME/.local/bin}"
    marker="$app_dir/.callus-app.json"
    installed_bin="$app_dir/callus"
    symlink_bin="$bin_dir/callus"

    action="install"
    case "${1:-}" in
        install|update|uninstall) action=$1; shift ;;
    esac

    dry_run=0
    for arg in "$@"; do
        if [ "$arg" = "--dry-run" ]; then dry_run=1; fi
    done

    run_with_tty() {
        if [ -r /dev/tty ] && (exec </dev/tty) 2>/dev/null; then
            "$@" </dev/tty
        else
            "$@" </dev/null
        fi
    }

    if [ "$action" = "uninstall" ]; then
        runner=""
        if [ -n "${CALLUS_BIN:-}" ] && [ -x "$CALLUS_BIN" ]; then
            runner="$CALLUS_BIN"
        elif [ -x "$installed_bin" ]; then
            runner="$installed_bin"
        elif command -v callus >/dev/null 2>&1; then
            runner="$(command -v callus)"
        fi
        if [ -n "$runner" ]; then
            run_with_tty "$runner" uninstall "$@"
        else
            echo "callus binary not found; skipping agent config cleanup."
        fi
        if [ -f "$marker" ]; then
            if [ "$dry_run" -eq 1 ]; then
                echo "[dry-run] would remove $symlink_bin and $app_dir"
            else
                if [ -L "$symlink_bin" ]; then rm -f "$symlink_bin"; fi
                rm -rf "$app_dir"
                echo "Removed $app_dir (your ~/.callus database was left untouched)."
            fi
        fi
        return 0
    fi

    if [ -d "$app_dir" ] && [ ! -f "$marker" ] && [ -n "$(ls -A "$app_dir" 2>/dev/null || true)" ]; then
        echo "error: $app_dir exists and was not created by the Callus installer; set CALLUS_APP to another folder." >&2
        exit 1
    fi

    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os/$arch" in
        Darwin/arm64|Darwin/aarch64) target="aarch64-apple-darwin" ;;
        Linux/x86_64|Linux/amd64)    target="x86_64-unknown-linux-gnu" ;;
        *)
            echo "No prebuilt archive for $os/$arch. Build from source with: cargo install --path crates/callus-cli --locked" >&2
            exit 1
            ;;
    esac

    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t callus)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    source_bin=""
    source_packs=""
    version_label="local"

    if [ -n "${CALLUS_BIN:-}" ] && [ -x "$CALLUS_BIN" ]; then
        source_bin="$CALLUS_BIN"
        cand_packs="$(dirname "$source_bin")/packs"
        if [ -d "$cand_packs" ]; then source_packs="$cand_packs"; fi
        echo "Using local binary $source_bin"
    else
        archive="$tmp/callus.tar.gz"
        if [ -n "${CALLUS_ARCHIVE:-}" ] && [ -f "$CALLUS_ARCHIVE" ]; then
            cp "$CALLUS_ARCHIVE" "$archive"
            version_label="archive"
        else
            ver="${CALLUS_VERSION:-}"
            if [ -z "$ver" ]; then
                echo "Checking latest Callus release from $repo..."
                if command -v curl >/dev/null 2>&1; then
                    ver="$(curl -fsSL -H 'User-Agent: callus-installer' "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1 || true)"
                elif command -v wget >/dev/null 2>&1; then
                    ver="$(wget -qO- "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1 || true)"
                else
                    echo "Neither curl nor wget is installed." >&2
                    exit 1
                fi
            fi
            if [ -z "$ver" ]; then
                echo "No published GitHub release found yet for $repo. Build from source with \`cargo install --path crates/callus-cli --locked\` or set CALLUS_BIN / CALLUS_ARCHIVE." >&2
                exit 1
            fi
            version_label="$ver"
            url="${CALLUS_ARCHIVE:-https://github.com/$repo/releases/download/$ver/callus-$ver-$target.tar.gz}"
            echo "Downloading $url"
            if command -v curl >/dev/null 2>&1; then
                curl -fsSL "$url" -o "$archive"
            else
                wget -q "$url" -O "$archive"
            fi
        fi
        tar -xzf "$archive" -C "$tmp"
        source_bin="$(find "$tmp" -maxdepth 3 -type f -name callus 2>/dev/null | head -n 1 || true)"
        if [ -z "$source_bin" ]; then
            echo "The downloaded archive did not contain the callus binary." >&2
            exit 1
        fi
        chmod +x "$source_bin"
        cand_packs="$(dirname "$source_bin")/packs"
        if [ -d "$cand_packs" ]; then source_packs="$cand_packs"; fi
    fi

    if [ "$dry_run" -eq 1 ]; then
        echo "[dry-run] would install callus to $installed_bin and symlink $symlink_bin"
        run_with_tty "$source_bin" "$action" "$@"
        return 0
    fi

    mkdir -p "$app_dir" "$bin_dir"
    cp "$source_bin" "$installed_bin"
    chmod +x "$installed_bin"
    if [ -n "$source_packs" ]; then
        rm -rf "$app_dir/packs"
        cp -R "$source_packs" "$app_dir/packs"
    fi
    ln -sf "$installed_bin" "$symlink_bin"
    printf '{\n  "managed_by": "callus-installer",\n  "version": "%s"\n}\n' "$version_label" > "$marker"
    echo "Installed Callus to $installed_bin (linked at $symlink_bin)"

    case ":${PATH:-}:" in
        *":$bin_dir:"*) ;;
        *) echo "Note: add $bin_dir to your PATH (e.g. export PATH=\"$bin_dir:\$PATH\")." ;;
    esac

    run_with_tty "$installed_bin" "$action" "$@"
}

# Everything is inside main so a partial download never runs half a script.
main "$@"

# chaosnexus-anvil/Justfile
# Retrityr-style multi-platform release compiles for the ChaosNexus Anvil MCP binary.
# Linux = native cargo; Windows / macOS = cargo-zigbuild (+ lipo for universal macOS).

# --- Variables ---
BIN_NAME := "chaosnexus-anvil"

# Absolute macOS SDK path for zigbuild / link (no `just` interpolation inside backticks).
# Order: <git-root>/SDKs/MacOSX.sdk, MACOSX_SDK_PATH, <git-root>/MacOSX.sdk, ~/development/macos-sdk/MacOSX.sdk
# Prefer private monorepo SDKs/MacOSX.sdk (Forgejo-only; never subtree-synced to Codeberg/GitHub).
MACOSX_SDK_ABS := `repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"; if [ -d "$repo/SDKs/MacOSX.sdk" ]; then c="$repo/SDKs/MacOSX.sdk"; elif [ -n "${MACOSX_SDK_PATH:-}" ] && [ -d "${MACOSX_SDK_PATH}" ]; then c="${MACOSX_SDK_PATH}"; elif [ -d "$repo/MacOSX.sdk" ]; then c="$repo/MacOSX.sdk"; elif [ -d "$HOME/development/macos-sdk/MacOSX.sdk" ]; then c="$HOME/development/macos-sdk/MacOSX.sdk"; else echo ""; exit 0; fi; case "$c" in /*) realpath "$c";; *) realpath "$repo/$c";; esac`

# Stage release binaries under monorepo artifacts/anvil/ (gitignored).
ARTIFACTS_DIR := `repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"; echo "$repo/artifacts/anvil"`

# Package version from this crate's Cargo.toml (first match only).
PKG_VERSION := `grep -m 1 '^version =' Cargo.toml | cut -d '"' -f 2`

# --- Performance & Scheduling ---
COMPILE_SCHED := "beerland"

CURRENT_SCHED := `if ! command -v scxctl >/dev/null 2>&1; then echo "unavailable"; elif scxctl get 2>/dev/null | grep -qi "no scx scheduler"; then echo "none"; else scxctl get 2>/dev/null | awk '{print tolower($2)}' || echo "unavailable"; fi`

RUSTC_WRAP := `if command -v sccache >/dev/null 2>&1; then echo "sccache"; else echo ""; fi`
MOLD_FLAG := `if command -v mold >/dev/null 2>&1; then echo "-C link-arg=-fuse-ld=mold"; else echo ""; fi`
LIPO_BIN := `if command -v llvm-lipo >/dev/null 2>&1; then echo "llvm-lipo"; elif command -v lipo >/dev/null 2>&1; then echo "lipo"; else echo ""; fi`

# Build profile defaults (overridden by the 'release' recipe)
PROFILE_FLAG := ""
TARGET_DIR := "debug"

# --- Recipes ---

default:
    @just --list

# Fail-fast check for requirements by selected platform
check-deps platform="all":
    #!/usr/bin/env bash
    set -euo pipefail
    p=$(echo "{{ platform }}" | tr '[:upper:]' '[:lower:]')
    echo "Checking hard prerequisites..."

    if ! command -v cargo >/dev/null 2>&1; then
        echo "Error: 'cargo' is not installed or not in PATH."
        exit 1
    fi

    case "$p" in
        all|windows|w|win|macos|mac|m|darwin)
            if ! command -v cargo-zigbuild >/dev/null 2>&1; then
                echo "Error: 'cargo-zigbuild' is required for $p builds."
                exit 1
            fi
            ;;
    esac

    case "$p" in
        all|windows|w|win)
            if [[ ! -f /usr/x86_64-w64-mingw32/lib/libsynchronization.a ]]; then
                echo "Error: missing mingw-w64 libsynchronization.a (needed for Zig 0.16 Windows link)."
                echo "   Install mingw-w64 CRT (e.g. Arch: mingw-w64-crt) so /usr/x86_64-w64-mingw32/lib/libsynchronization.a exists."
                exit 1
            fi
            ;;
    esac

    case "$p" in
        all|macos|mac|m|darwin)
            if [[ -z "{{LIPO_BIN}}" ]]; then
                echo "Error: neither 'llvm-lipo' nor 'lipo' was found (required for macOS universal binaries)."
                exit 1
            fi
            if [[ -z "{{MACOSX_SDK_ABS}}" ]] || [[ ! -d "{{MACOSX_SDK_ABS}}" ]]; then
                echo "Error: macOS SDK not found."
                echo "   Looked for: \$MACOSX_SDK_PATH, \$(git rev-parse --show-toplevel)/MacOSX.sdk, and $HOME/development/macos-sdk/MacOSX.sdk"
                exit 1
            fi
            ;;
    esac

    echo "  -> All core tools found!"

# DEBUG build: default all platforms; or one of linux / windows / macos
debug platform="all":
    #!/usr/bin/env bash
    set -euo pipefail

    just check-deps "{{platform}}"

    # Graceful degradation: only touch the scheduler if scxctl + passwordless sudo work.
    if [ "{{CURRENT_SCHED}}" != "unavailable" ]; then
        echo "Optimizing CPU scheduler -> scx_{{COMPILE_SCHED}}..."
        if sudo -n scxctl switch --sched {{COMPILE_SCHED}} 2>/dev/null; then
            if [ "{{CURRENT_SCHED}}" = "none" ]; then
                trap 'echo "Restoring CPU scheduler -> SCX OFF..."; sudo -n scxctl stop 2>/dev/null || true' EXIT
            else
                trap 'echo "Restoring CPU scheduler -> scx_{{CURRENT_SCHED}}..."; sudo -n scxctl switch --sched {{CURRENT_SCHED}} 2>/dev/null || true' EXIT
            fi
        fi
    fi

    p=$(echo "{{ platform }}" | tr '[:upper:]' '[:lower:]')
    case "$p" in
        all)
            just build-all
            ;;
        linux|l)
            just build-linux
            just stage-artifacts linux
            echo "Linux ({{TARGET_DIR}}) build + stage complete!"
            ;;
        windows|w|win)
            just build-windows
            just stage-artifacts windows
            echo "Windows ({{TARGET_DIR}}) build + stage complete!"
            ;;
        macos|mac|m|darwin)
            just build-macos
            just stage-artifacts macos
            echo "macOS universal ({{TARGET_DIR}}) build + stage complete!"
            ;;
        *)
            echo "error: unknown platform '$p'" >&2
            exit 1
            ;;
    esac

# Sync is a no-op for Anvil (version lives only in Cargo.toml); kept for parity with Retrityr.
sync-version:
    @echo "Anvil version: {{PKG_VERSION}} (from Cargo.toml)"

# RELEASE build: all platforms + stage under artifacts/anvil/
release: sync-version
    #!/usr/bin/env bash
    set -euo pipefail

    just check-deps all

    if [ "{{CURRENT_SCHED}}" != "unavailable" ]; then
        echo "Optimizing CPU scheduler -> scx_{{COMPILE_SCHED}}..."
        if sudo -n scxctl switch --sched {{COMPILE_SCHED}} 2>/dev/null; then
            if [ "{{CURRENT_SCHED}}" = "none" ]; then
                trap 'echo "Restoring CPU scheduler -> SCX OFF..."; sudo -n scxctl stop 2>/dev/null || true' EXIT
            else
                trap 'echo "Restoring CPU scheduler -> scx_{{CURRENT_SCHED}}..."; sudo -n scxctl switch --sched {{CURRENT_SCHED}} 2>/dev/null || true' EXIT
            fi
        fi
    fi

    just PROFILE_FLAG="--release" TARGET_DIR="release" build-all

build-all: build-macos build-linux build-windows stage-artifacts
    @echo "All platforms compiled successfully in {{TARGET_DIR}} mode!"

# [1/3] Compile for Linux (Native)
build-linux:
    @echo "Building for Linux (x86_64 native | {{TARGET_DIR}})..."
    env RUSTC_WRAPPER={{RUSTC_WRAP}} RUSTFLAGS="{{MOLD_FLAG}}" cargo build {{PROFILE_FLAG}}

# [2/3] Compile for Windows (GNU) via Zigbuild
# Zig 0.16 does not ship an import lib for Synchronization.dll; point the linker
# at mingw-w64's libsynchronization.a (sysinfo / windows-sys pull -lsynchronization).
MINGW64_LIB := `if [ -d /usr/x86_64-w64-mingw32/lib ]; then echo /usr/x86_64-w64-mingw32/lib; else echo ""; fi`

build-windows:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Building for Windows (x86_64-pc-windows-gnu | {{TARGET_DIR}})..."
    echo "  -> zig version: $(zig version 2>/dev/null || echo 'not found')"
    echo "  -> cargo-zigbuild version: $(cargo-zigbuild -V 2>/dev/null || cargo zigbuild -V 2>/dev/null || echo 'not found')"
    # rustc passes -O to the linker; Zig ignores it and prints
    # "ignoring deprecated linker optimization setting '1'" (rust-lang/rust#158192).
    # Allow only on zigbuild targets so native Linux still surfaces real linker noise.
    extra_rustflags=""
    if [[ -n "{{MINGW64_LIB}}" ]] && [[ -f "{{MINGW64_LIB}}/libsynchronization.a" ]]; then
        extra_rustflags="${extra_rustflags} -Lnative={{MINGW64_LIB}}"
        echo "  -> using mingw sync import lib at {{MINGW64_LIB}}"
    else
        echo "Warning: {{MINGW64_LIB}}/libsynchronization.a not found; Windows link may fail on -lsynchronization"
    fi
    env RUSTC_WRAPPER={{RUSTC_WRAP}} \
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="${extra_rustflags}" \
        cargo zigbuild --target x86_64-pc-windows-gnu {{PROFILE_FLAG}}

# [3/3] Compile for macOS (Universal) via Zigbuild
build-macos:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Building for macOS (Universal Binary | {{TARGET_DIR}})..."
    if [ -z "{{MACOSX_SDK_ABS}}" ] || [ ! -d "{{MACOSX_SDK_ABS}}" ]; then
        echo "Error: macOS SDK not found."
        echo "   Set MACOSX_SDK_PATH or place SDK at repo root: MacOSX.sdk"
        exit 1
    fi
    if [ -z "{{LIPO_BIN}}" ]; then
        echo "Error: missing lipo tool ('llvm-lipo' or 'lipo')."
        exit 1
    fi
    echo "  -> Using SDKROOT={{MACOSX_SDK_ABS}}"
    echo "  -> Using lipo tool={{LIPO_BIN}}"
    echo "  -> zig version: $(zig version 2>/dev/null || echo 'not found')"
    echo "  -> cargo-zigbuild version: $(cargo-zigbuild -V 2>/dev/null || cargo zigbuild -V 2>/dev/null || echo 'not found')"
    # Zig cannot parse Apple TBD text-redirect aliases; convert to symlinks.
    if [ -f "$(git rev-parse --show-toplevel 2>/dev/null)/tools/deploy/fixup-macos-sdk-tbds.sh" ]; then
        bash "$(git rev-parse --show-toplevel)/tools/deploy/fixup-macos-sdk-tbds.sh" "{{MACOSX_SDK_ABS}}" || true
    fi
    zig_rustflags=""
    echo "  -> Compiling Apple Silicon (aarch64)..."
    ziglog="$(mktemp)"
    set +e
    env RUSTC_WRAPPER={{RUSTC_WRAP}} SDKROOT={{MACOSX_SDK_ABS}} \
        CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="${zig_rustflags}" \
        cargo zigbuild --target aarch64-apple-darwin {{PROFILE_FLAG}} -vv 2>&1 | tee "$ziglog"
    rc=${PIPESTATUS[0]}
    set -e
    if [[ "$rc" -ne 0 ]]; then
        echo "--- ZIGBUILD aarch64 FAILED (exit $rc) — last 200 lines of verbose output: ---"
        tail -200 "$ziglog"
        rm -f "$ziglog"
        exit "$rc"
    fi
    rm -f "$ziglog"
    echo "  -> Compiling Intel (x86_64)..."
    env RUSTC_WRAPPER={{RUSTC_WRAP}} SDKROOT={{MACOSX_SDK_ABS}} \
        CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="${zig_rustflags}" \
        cargo zigbuild --target x86_64-apple-darwin {{PROFILE_FLAG}}
    echo "  -> Stitching into Universal Binary..."
    mkdir -p "target/{{TARGET_DIR}}"
    {{LIPO_BIN}} -create \
        -output "target/{{TARGET_DIR}}/{{BIN_NAME}}_universal" \
        "target/aarch64-apple-darwin/{{TARGET_DIR}}/{{BIN_NAME}}" \
        "target/x86_64-apple-darwin/{{TARGET_DIR}}/{{BIN_NAME}}"

# Copy compiled binaries into monorepo artifacts/anvil/ with stable names.
stage-artifacts which="all":
    #!/usr/bin/env bash
    set -euo pipefail
    w=$(echo "{{which}}" | tr '[:upper:]' '[:lower:]')
    ver="{{PKG_VERSION}}"
    out="{{ARTIFACTS_DIR}}"
    mkdir -p "$out"
    echo "Staging artifacts to $out (version $ver | {{TARGET_DIR}})..."

    LINUX_SRC="target/{{TARGET_DIR}}/{{BIN_NAME}}"
    WIN_SRC="target/x86_64-pc-windows-gnu/{{TARGET_DIR}}/{{BIN_NAME}}.exe"
    MAC_SRC="target/{{TARGET_DIR}}/{{BIN_NAME}}_universal"

    LINUX_DST="$out/{{BIN_NAME}}-${ver}-x86_64-unknown-linux-gnu"
    WIN_DST="$out/{{BIN_NAME}}-${ver}-x86_64-pc-windows-gnu.exe"
    MAC_DST="$out/{{BIN_NAME}}-${ver}-universal-apple-darwin"

    copy_required() {
        local src="$1" dst="$2"
        if [[ ! -f "$src" ]]; then
            echo "error: expected artifact missing: $src" >&2
            exit 1
        fi
        install -p "$src" "$dst"
        echo "  -> staged $(basename "$dst")"
    }

    copy_if_exists() {
        local src="$1" dst="$2"
        if [[ -f "$src" ]]; then
            install -p "$src" "$dst"
            echo "  -> staged $(basename "$dst")"
        fi
    }

    case "$w" in
        all)
            copy_if_exists "$LINUX_SRC" "$LINUX_DST"
            copy_if_exists "$WIN_SRC" "$WIN_DST"
            copy_if_exists "$MAC_SRC" "$MAC_DST"
            ;;
        linux|l)
            copy_required "$LINUX_SRC" "$LINUX_DST"
            ;;
        windows|w|win)
            copy_required "$WIN_SRC" "$WIN_DST"
            ;;
        macos|mac|m|darwin)
            copy_required "$MAC_SRC" "$MAC_DST"
            ;;
        *)
            echo "error: unknown stage-artifacts target '$w' — use all, linux, windows, or macos" >&2
            exit 1
            ;;
    esac
    echo "  -> Stage step done!"

# Legacy local recipes (unchanged entry points)
build:
    cargo build

test:
    cargo test

clippy:
    cargo clippy

clean:
    cargo clean

run *args:
    cargo run -- {{args}}

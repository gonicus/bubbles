#!/bin/bash
#
# prebuild.bash — Produce prebuilt/ artifacts for Flatpak builds
#
# Builds crosvm (with patches), extracts socat + qemu-img from Debian Trixie,
# and generates cargo-sources.json for offline Cargo builds. This script is
# the single source of truth — both local builds and CI use it.
#
# Usage:
#   ./prebuild.bash                        # Build everything (crosvm from source)
#   CROSVM=~/bubbles/crosvm ./prebuild.bash  # Use pre-built crosvm binary
#
# Environment variables:
#   CROSVM  - path to a pre-built crosvm binary (skips crosvm build)
#
# Requirements: podman, git, sha256sum

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PREBUILT_DIR="$SCRIPT_DIR/prebuilt"
PATCHES_DIR="$SCRIPT_DIR/patches/crosvm"

# --- crosvm source configuration ---
CROSVM_COMMIT="a96cb379acf55a75887cbba190666e7d22ff9dbf"
VIRGLRENDERER_COMMIT="ca50e008863837e094747a69974dde3ae148aeaa"
CROSVM_REVERTS=(
    1656a1f68296baa4313b4b46e23a6c975caa7cc9
    2c6f23406c41af8432c1c1ba4e3605785e959ead
    806e91d2fa5416b3444257e42421e07b318e26d6
    ff4b721ac8b983393b0fa503000eff74ecd3de2e
    a96cb379acf55a75887cbba190666e7d22ff9dbf
)

# --- Container names (for cleanup) ---
TOOLS_CONTAINER="bubbles-prebuild-tools-$$"
CROSVM_CONTAINER="bubbles-prebuild-crosvm-$$"
TMPDIR_CROSVM=""

cleanup() {
    podman rm -f "$TOOLS_CONTAINER" 2>/dev/null || true
    podman rm -f "$CROSVM_CONTAINER" 2>/dev/null || true
    if [ -n "$TMPDIR_CROSVM" ] && [ -d "$TMPDIR_CROSVM" ]; then
        rm -rf "$TMPDIR_CROSVM"
    fi
}
trap cleanup EXIT

mkdir -p "$PREBUILT_DIR" "$PREBUILT_DIR/lib"

# ---------------------------------------------------------------------------
# crosvm — build from source or copy pre-built binary
# ---------------------------------------------------------------------------
crosvm_cache_key() {
    local reverts_str
    reverts_str=$(printf '%s\n' "${CROSVM_REVERTS[@]}")
    local patches_hash
    patches_hash=$(cat "$PATCHES_DIR"/*.patch 2>/dev/null | sha256sum | awk '{print $1}')
    echo -n "${CROSVM_COMMIT}:${VIRGLRENDERER_COMMIT}:${reverts_str}:${patches_hash}" | sha256sum | awk '{print $1}'
}

if [ -n "${CROSVM:-}" ]; then
    echo "==> crosvm: copying from ${CROSVM}"
    install -m755 "$CROSVM" "$PREBUILT_DIR/crosvm"
    echo "    → prebuilt/crosvm"
else
    CACHE_KEY=$(crosvm_cache_key)
    CACHE_FILE="$PREBUILT_DIR/.crosvm-cache-key"

    if [ -f "$PREBUILT_DIR/crosvm" ] && [ -f "$CACHE_FILE" ] && [ "$(cat "$CACHE_FILE")" = "$CACHE_KEY" ]; then
        echo "==> crosvm: cached, skipping (key: ${CACHE_KEY:0:12}…)"
    else
        echo "==> crosvm: building from source (commit ${CROSVM_COMMIT:0:12}…)"
        TMPDIR_CROSVM=$(mktemp -d)

        echo "    Cloning crosvm..."
        git clone --quiet https://chromium.googlesource.com/crosvm/crosvm "$TMPDIR_CROSVM/crosvm"
        cd "$TMPDIR_CROSVM/crosvm"

        git config user.email "prebuild@bubbles"
        git config user.name "prebuild"
        git checkout --quiet "$CROSVM_COMMIT"

        echo "    Reverting commits..."
        git revert --no-edit "${CROSVM_REVERTS[@]}"

        echo "    Applying patches..."
        for patch in "$PATCHES_DIR"/*.patch; do
            echo "      $(basename "$patch")"
            git apply "$patch"
        done

        echo "    Initializing submodules..."
        git submodule update --init

        echo "    Cloning virglrenderer..."
        git init "$TMPDIR_CROSVM/virglrenderer"
        git -C "$TMPDIR_CROSVM/virglrenderer" fetch --depth=1 \
            https://gitlab.freedesktop.org/virgl/virglrenderer.git "$VIRGLRENDERER_COMMIT"
        git -C "$TMPDIR_CROSVM/virglrenderer" checkout FETCH_HEAD

        echo "    Building in container (this may take a while)..."
        podman run -d --name "$CROSVM_CONTAINER" \
            -v "$TMPDIR_CROSVM/crosvm:/src:Z" \
            -v "$TMPDIR_CROSVM/virglrenderer:/virglrenderer:Z" \
            rust:trixie sleep infinity

        # Build virglrenderer first (with amdgpu-experimental DRM renderer)
        podman exec "$CROSVM_CONTAINER" bash -c '
            apt-get update
            apt-get install -y meson ninja-build libgbm-dev libdrm-dev libepoxy-dev pkg-config python3-yaml
            cd /virglrenderer
            meson setup builddir --prefix=/usr/local \
                -Dvenus=true -Dplatforms=egl -Ddrm-renderers=amdgpu-experimental
            ninja -C builddir
            DESTDIR=/opt/virglrenderer-install ninja -C builddir install
            find /opt/virglrenderer-install -name virglrenderer.pc \
                -exec sed -i "s|^prefix=.*|prefix=/opt/virglrenderer-install/usr/local|" {} \;
        '

        # Detect the library directory virglrenderer was installed into
        VIRGL_LIBDIR=$(podman exec "$CROSVM_CONTAINER" \
            find /opt/virglrenderer-install/usr/local -name 'libvirglrenderer.so' -printf '%h' -quit)

        # Build crosvm with virgl_renderer feature
        podman exec "$CROSVM_CONTAINER" bash -c "
            cd /src
            sed -i 's/sudo //' tools/deps/install-x86_64-debs && tools/deps/install-x86_64-debs
            PKG_CONFIG_PATH=${VIRGL_LIBDIR}/pkgconfig \
            LD_LIBRARY_PATH=${VIRGL_LIBDIR} \
            cargo build --release --features virgl_renderer
        "

        podman cp "$CROSVM_CONTAINER:/src/target/release/crosvm" "$PREBUILT_DIR/crosvm"

        # Copy virglrenderer libraries
        echo "    Copying virglrenderer libraries..."
        podman exec "$CROSVM_CONTAINER" bash -c "ls ${VIRGL_LIBDIR}/" | while read -r f; do
            podman cp "$CROSVM_CONTAINER:${VIRGL_LIBDIR}/$f" "$PREBUILT_DIR/lib/$f"
            echo "    → prebuilt/lib/$f"
        done

        # Copy crosvm runtime library dependencies (e.g. libwayland-client, libcap)
        echo "    Copying crosvm runtime libraries..."
        CROSVM_DEPS=$(podman exec "$CROSVM_CONTAINER" bash -c "
            ldd /src/target/release/crosvm 2>/dev/null \
            | grep '=> /' | awk '{print \$3}' | sort -u")
        for lib in $CROSVM_DEPS; do
            libname=$(basename "$lib")
            # Skip libs already copied (virglrenderer, vDSO, libc basics already bundled by tools step)
            if [ -f "$PREBUILT_DIR/lib/$libname" ]; then
                continue
            fi
            podman cp "$CROSVM_CONTAINER:$lib" "$PREBUILT_DIR/lib/$libname"
            echo "    → prebuilt/lib/$libname"
        done

        chmod +x "$PREBUILT_DIR/crosvm"
        echo "$CACHE_KEY" > "$CACHE_FILE"

        cd "$SCRIPT_DIR"
        echo "    → prebuilt/crosvm"
    fi
fi

# ---------------------------------------------------------------------------
# socat, qemu-img, and runtime libraries — install in Debian Trixie container
# via apt (verifies package GPG signatures), then copy binaries and their
# non-system shared library dependencies out.
# ---------------------------------------------------------------------------
echo "==> Setting up Debian Trixie container..."

podman run -d --name "$TOOLS_CONTAINER" debian:trixie sleep infinity
podman exec "$TOOLS_CONTAINER" sh -c \
    'apt-get update && apt-get install -y --no-install-recommends socat qemu-utils'

# Copy binaries
echo "==> Copying binaries..."
podman cp "$TOOLS_CONTAINER:/usr/bin/socat1"   "$PREBUILT_DIR/socat"
podman cp "$TOOLS_CONTAINER:/usr/bin/qemu-img" "$PREBUILT_DIR/qemu-img"
chmod +x "$PREBUILT_DIR/socat" "$PREBUILT_DIR/qemu-img"
echo "    → prebuilt/socat"
echo "    → prebuilt/qemu-img"

# Copy runtime library dependencies (including glibc for non-FHS distro support)
echo "==> Copying runtime libraries..."

# Only skip the kernel-injected vDSO — bundle everything else including glibc,
# so binaries work on non-FHS distros (NixOS, Guix) where /lib64/ doesn't exist.
SYSTEM_LIBS="linux-vdso"

DEPS=$(podman exec "$TOOLS_CONTAINER" sh -c \
    'ldd /usr/bin/socat1 /usr/bin/qemu-img 2>/dev/null \
     | grep "=> /" | awk "{print \$3}" | sort -u')

for lib in $DEPS; do
    libname=$(basename "$lib")
    if echo "$libname" | grep -qE "$SYSTEM_LIBS"; then
        continue
    fi
    podman cp "$TOOLS_CONTAINER:$lib" "$PREBUILT_DIR/lib/$libname"
    echo "    → prebuilt/lib/$libname"
done

# Bundle the dynamic linker itself (not captured by ldd grep pattern)
podman cp "$TOOLS_CONTAINER:/lib64/ld-linux-x86-64.so.2" "$PREBUILT_DIR/lib/ld-linux-x86-64.so.2"
chmod +x "$PREBUILT_DIR/lib/ld-linux-x86-64.so.2"
echo "    → prebuilt/lib/ld-linux-x86-64.so.2"

# ---------------------------------------------------------------------------
# License collection — gather license/copyright files for all bundled components
# ---------------------------------------------------------------------------
echo "==> Collecting licenses..."

LICENSES_DIR="$PREBUILT_DIR/licenses"
rm -rf "$LICENSES_DIR"
mkdir -p "$LICENSES_DIR/crosvm" "$LICENSES_DIR/debian"

# crosvm (BSD-3-Clause) — already checked into the repo
cp "$PATCHES_DIR/LICENSE" "$LICENSES_DIR/crosvm/LICENSE"
echo "    → licenses/crosvm/LICENSE"

# socat and qemu-img — extract copyright + source info
# Map binary names to their Debian package names (they don't always match)
declare -A BIN_TO_PKG=( [socat]=socat [qemu-img]=qemu-utils )

for bin_name in socat qemu-img; do
    deb_pkg="${BIN_TO_PKG[$bin_name]}"
    pkg_dir="$LICENSES_DIR/debian/$bin_name"
    mkdir -p "$pkg_dir"

    # Extract Debian copyright file (keyed by Debian package name)
    podman cp "$TOOLS_CONTAINER:/usr/share/doc/$deb_pkg/copyright" "$pkg_dir/copyright" 2>/dev/null || \
        podman exec "$TOOLS_CONTAINER" sh -c "cat /usr/share/doc/${deb_pkg}/copyright" > "$pkg_dir/copyright"

    # Get package version for the source offer
    pkg_version=$(podman exec "$TOOLS_CONTAINER" dpkg-query -W -f '${Version}' "$deb_pkg")

    cat > "$pkg_dir/SOURCE-INFO" <<INFO
Source: $bin_name (Debian package: $deb_pkg)
Version: $pkg_version
Origin: Debian Trixie package archive
Source package: https://packages.debian.org/source/trixie/$deb_pkg
INFO

    echo "    → licenses/debian/$bin_name/copyright"
    echo "    → licenses/debian/$bin_name/SOURCE-INFO"
done

# Runtime library dependencies — map each .so back to its Debian package
echo "    Collecting runtime library copyrights..."
LIB_PKGS=$(podman exec "$TOOLS_CONTAINER" sh -c '
    for lib in /usr/bin/socat1 /usr/bin/qemu-img; do
        ldd "$lib" 2>/dev/null
    done | grep "=> /" | awk "{print \$3}" | sort -u | while read -r libpath; do
        dpkg -S "$libpath" 2>/dev/null | cut -d: -f1
    done | sort -u
')

for pkg in $LIB_PKGS; do
    # Skip packages we already handle directly
    case "$pkg" in
        socat|qemu-utils) continue ;;
    esac

    pkg_dir="$LICENSES_DIR/debian/$pkg"
    mkdir -p "$pkg_dir"
    podman cp "$TOOLS_CONTAINER:/usr/share/doc/$pkg/copyright" "$pkg_dir/copyright" 2>/dev/null || \
        podman exec "$TOOLS_CONTAINER" sh -c "cat /usr/share/doc/${pkg}/copyright" > "$pkg_dir/copyright" 2>/dev/null || \
        echo "    Warning: no copyright file found for $pkg"
    echo "    → licenses/debian/$pkg/copyright"
done

# ---------------------------------------------------------------------------
# cargo-sources.json — Flatpak needs this for offline Cargo builds
# Run generator inside the container using apt-provided Python packages
# (avoids needing pip/aiohttp on the host).
# ---------------------------------------------------------------------------
if [ -f "$SCRIPT_DIR/cargo-sources.json" ]; then
    echo "==> cargo-sources.json already exists, skipping"
else
    echo "==> Generating cargo-sources.json (inside container)..."
    podman exec "$TOOLS_CONTAINER" sh -c \
        'apt-get install -y --no-install-recommends python3 python3-aiohttp python3-tomlkit curl 2>/dev/null'
    curl -fsSL -o "$SCRIPT_DIR/.flatpak-cargo-generator.py" \
        https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/4d5e760321236bd96fc1c6db9ec94c336600c114/cargo/flatpak-cargo-generator.py
    podman cp "$SCRIPT_DIR/Cargo.lock"                    "$TOOLS_CONTAINER:/tmp/Cargo.lock"
    podman cp "$SCRIPT_DIR/.flatpak-cargo-generator.py"   "$TOOLS_CONTAINER:/tmp/flatpak-cargo-generator.py"
    rm -f "$SCRIPT_DIR/.flatpak-cargo-generator.py"
    podman exec "$TOOLS_CONTAINER" \
        python3 /tmp/flatpak-cargo-generator.py /tmp/Cargo.lock -o /tmp/cargo-sources.json
    podman cp "$TOOLS_CONTAINER:/tmp/cargo-sources.json" "$SCRIPT_DIR/cargo-sources.json"
    echo "    → cargo-sources.json"
fi

# ---------------------------------------------------------------------------
echo ""
echo "prebuilt/ ready:"
ls -lhR "$PREBUILT_DIR/"
echo ""
echo "To build the Flatpak:"
echo "  cd $(basename "$SCRIPT_DIR")"
echo "  flatpak-builder --user --install --force-clean build-dir de.gonicus.Bubbles.json"

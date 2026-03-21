#!/usr/bin/env bash
set -euo pipefail
umask 022

BINARY_NAME="why"
OWNER="quangdang46"
REPO="why"
DEST="${DEST:-$HOME/.local/bin}"
VERSION="${VERSION:-}"
QUIET=0
EASY=0
VERIFY=0
FROM_SOURCE=0
UNINSTALL=0
MAX_RETRIES=3
DOWNLOAD_TIMEOUT=120
LOCK_DIR="/tmp/${BINARY_NAME}-install.lock.d"
TMP=""

log_info() {
    [ "$QUIET" -eq 1 ] && return 0
    echo "[${BINARY_NAME}] $*" >&2
}

log_warn() {
    echo "[${BINARY_NAME}] WARN: $*" >&2
}

log_success() {
    [ "$QUIET" -eq 1 ] && return 0
    echo "✓ $*" >&2
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

usage() {
    cat <<EOF
Install ${BINARY_NAME} from GitHub releases.

Usage: install.sh [options]

Options:
  --dest <dir>         Install into a custom directory
  --dest=<dir>         Install into a custom directory
  --version <tag>      Install a specific release tag
  --version=<tag>      Install a specific release tag
  --system             Install into /usr/local/bin
  --easy-mode          Add install directory to shell rc files when needed
  --verify             Run ${BINARY_NAME} --version after install
  --from-source        Build from source instead of downloading a release
  --quiet, -q          Reduce non-error output
  --uninstall          Remove installed binary
  -h, --help           Show this help
EOF
    exit 0
}

cleanup() {
    rm -rf "$TMP" "$LOCK_DIR" 2>/dev/null || true
}

trap cleanup EXIT

acquire_lock() {
    if mkdir "$LOCK_DIR" 2>/dev/null; then
        echo $$ > "$LOCK_DIR/pid"
        return 0
    fi
    die "Another install is running. If stuck: rm -rf $LOCK_DIR"
}

remove_installer_path_lines() {
    local rc="$1"
    [ -f "$rc" ] || return 0

    local tmp_file
    tmp_file=$(mktemp "${TMPDIR:-/tmp}/${BINARY_NAME}-rc.XXXXXX")
    grep -vF "# ${BINARY_NAME} installer" "$rc" > "$tmp_file" || true
    cat "$tmp_file" > "$rc"
    rm -f "$tmp_file"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dest)
            [ $# -ge 2 ] || die "Missing value for --dest"
            DEST="$2"
            shift 2
            ;;
        --dest=*)
            DEST="${1#*=}"
            shift
            ;;
        --version)
            [ $# -ge 2 ] || die "Missing value for --version"
            VERSION="$2"
            shift 2
            ;;
        --version=*)
            VERSION="${1#*=}"
            shift
            ;;
        --system)
            DEST="/usr/local/bin"
            shift
            ;;
        --easy-mode)
            EASY=1
            shift
            ;;
        --verify)
            VERIFY=1
            shift
            ;;
        --from-source)
            FROM_SOURCE=1
            shift
            ;;
        --quiet|-q)
            QUIET=1
            shift
            ;;
        --uninstall)
            UNINSTALL=1
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            die "Unknown argument: $1"
            ;;
    esac
done

do_uninstall() {
    rm -f "$DEST/$BINARY_NAME" "$DEST/$BINARY_NAME.exe"
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        remove_installer_path_lines "$rc"
    done
    log_success "Uninstalled"
    exit 0
}

[ "$UNINSTALL" -eq 1 ] && do_uninstall

detect_platform() {
    local os arch
    case "$(uname -s)" in
        Linux*)
            os="linux"
            ;;
        Darwin*)
            os="macos"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            os="windows"
            ;;
        *)
            die "Unsupported OS: $(uname -s)"
            ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64)
            arch="x86_64"
            ;;
        aarch64|arm64)
            arch="aarch64"
            ;;
        *)
            die "Unsupported arch: $(uname -m)"
            ;;
    esac
    echo "${os}_${arch}"
}

archive_name_for_platform() {
    local platform="$1"
    case "$platform" in
        linux_x86_64)
            echo "${BINARY_NAME}-${VERSION}-linux-x86_64.tar.gz"
            ;;
        linux_aarch64)
            echo "${BINARY_NAME}-${VERSION}-linux-aarch64.tar.gz"
            ;;
        macos_x86_64)
            echo "${BINARY_NAME}-${VERSION}-macos-x86_64.tar.gz"
            ;;
        macos_aarch64)
            echo "${BINARY_NAME}-${VERSION}-macos-aarch64.tar.gz"
            ;;
        windows_x86_64)
            echo "${BINARY_NAME}-${VERSION}-windows-x86_64.zip"
            ;;
        *)
            die "Unsupported platform mapping: $platform"
            ;;
    esac
}

installed_binary_name_for_platform() {
    local platform="$1"
    case "$platform" in
        windows_*)
            echo "${BINARY_NAME}.exe"
            ;;
        *)
            echo "$BINARY_NAME"
            ;;
    esac
}

resolve_version() {
    [ -n "$VERSION" ] && return 0

    VERSION=$(curl -fsSL \
        --connect-timeout 10 --max-time 30 \
        -H "Accept: application/vnd.github.v3+json" \
        "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" \
        2>/dev/null | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/') || true

    if [ -z "$VERSION" ]; then
        VERSION=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
            "https://github.com/${OWNER}/${REPO}/releases/latest" \
            2>/dev/null | sed -E 's|.*/tag/||') || true
    fi

    [[ "$VERSION" =~ ^v[0-9] ]] || die "Could not resolve version"
    log_info "Latest: $VERSION"
}

download_file() {
    local url="$1"
    local dest="$2"
    local partial="${dest}.part"
    local attempt=0

    while [ $attempt -lt $MAX_RETRIES ]; do
        attempt=$((attempt + 1))
        if curl -fL \
            --connect-timeout 30 \
            --max-time "$DOWNLOAD_TIMEOUT" \
            --retry 2 \
            $( [ -s "$partial" ] && echo "--continue-at -" ) \
            $( [ "$QUIET" -eq 0 ] && [ -t 2 ] && echo "--progress-bar" || echo "-sS" ) \
            -o "$partial" "$url"; then
            mv -f "$partial" "$dest"
            return 0
        fi
        if [ $attempt -lt $MAX_RETRIES ]; then
            log_warn "Retrying in 3s..."
            sleep 3
        fi
    done
    return 1
}

checksum_cmd() {
    if command -v sha256sum >/dev/null 2>&1; then
        echo "sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        echo "shasum -a 256"
    else
        die "No SHA-256 checksum utility found"
    fi
}

install_binary_atomic() {
    local src="$1"
    local dest="$2"
    local tmp="${dest}.tmp.$$"
    install -m 0755 "$src" "$tmp"
    mv -f "$tmp" "$dest" || {
        rm -f "$tmp"
        die "Failed to install binary"
    }
}

maybe_add_path() {
    case ":$PATH:" in
        *":$DEST:"*) return 0 ;;
    esac

    if [ "$EASY" -eq 1 ]; then
        for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
            [ -f "$rc" ] && [ -w "$rc" ] || continue
            grep -qF "$DEST" "$rc" && continue
            printf '\nexport PATH="%s:$PATH"  # %s installer\n' "$DEST" "$BINARY_NAME" >> "$rc"
        done
        log_warn "PATH updated — restart shell or: export PATH=\"$DEST:\$PATH\""
    else
        log_warn "Add to PATH: export PATH=\"$DEST:\$PATH\""
    fi
}

build_from_source() {
    local installed_name="$1"

    command -v cargo >/dev/null || die "Rust/cargo not found. Install: https://rustup.rs"
    command -v git >/dev/null || die "git not found"
    if [ -n "$VERSION" ]; then
        git clone --depth 1 --branch "$VERSION" "https://github.com/${OWNER}/${REPO}.git" "$TMP/src"
    else
        git clone --depth 1 "https://github.com/${OWNER}/${REPO}.git" "$TMP/src"
    fi
    (
        cd "$TMP/src"
        CARGO_TARGET_DIR="$TMP/target" cargo build --release --locked --package why-core --bin "$BINARY_NAME"
    )
    install_binary_atomic "$TMP/target/release/$installed_name" "$DEST/$installed_name"
}

print_summary() {
    local installed_path="$1"

    echo ""
    echo "✓ ${BINARY_NAME} installed → $installed_path"
    echo "  Version: $("$installed_path" --version 2>/dev/null || echo 'unknown')"
    echo ""
    echo "  Quick start:"
    echo "    $installed_path --help"
}

main() {
    acquire_lock
    TMP=$(mktemp -d)
    mkdir -p "$DEST"

    local platform archive url checksum_url expected actual checksum_tool installed_name installed_path bin_path
    platform=$(detect_platform)
    installed_name=$(installed_binary_name_for_platform "$platform")
    installed_path="$DEST/$installed_name"
    log_info "Platform: $platform | Dest: $DEST"

    if [ "$FROM_SOURCE" -eq 0 ]; then
        resolve_version
        archive=$(archive_name_for_platform "$platform")
        url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${archive}"

        if download_file "$url" "$TMP/$archive"; then
            checksum_url="${url}.sha256"
            if download_file "$checksum_url" "$TMP/checksum.sha256" 2>/dev/null; then
                expected=$(awk '{print $1}' "$TMP/checksum.sha256")
                checksum_tool=$(checksum_cmd)
                actual=$($checksum_tool "$TMP/$archive" | awk '{print $1}')
                [ "$expected" = "$actual" ] || die "Checksum mismatch"
                log_info "Checksum verified"
            fi

            case "$archive" in
                *.tar.gz)
                    tar -xzf "$TMP/$archive" -C "$TMP"
                    ;;
                *.zip)
                    if command -v unzip >/dev/null 2>&1; then
                        unzip -q "$TMP/$archive" -d "$TMP"
                    else
                        die "unzip is required to install Windows archives"
                    fi
                    ;;
            esac

            bin_path=$(find "$TMP" -type f \( -name "$installed_name" -o -name "$BINARY_NAME" \) | head -n 1)
            [ -n "$bin_path" ] || die "Binary not found after extract"
            install_binary_atomic "$bin_path" "$installed_path"
        else
            log_warn "Binary download failed — building from source..."
            build_from_source "$installed_name"
        fi
    else
        build_from_source "$installed_name"
    fi

    maybe_add_path

    if [ "$VERIFY" -eq 1 ]; then
        "$installed_path" --version
    fi

    print_summary "$installed_path"
}

if [[ "${BASH_SOURCE[0]:-}" == "${0:-}" ]] || [[ -z "${BASH_SOURCE[0]:-}" ]]; then
    { main "$@"; }
fi

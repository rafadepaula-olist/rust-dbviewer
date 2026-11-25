#!/usr/bin/env bash
set -euo pipefail

REPO="${RUST_DBVIEWER_REPO:-rafadepaula-olist/rust-dbviewer}"
ASSET_NAME="rust-dbviewer"
CHECKSUM_ASSET="${ASSET_NAME}.sha256"
USE_LOCAL=0

for arg in "$@"; do
    case "${arg}" in
        --local)
            USE_LOCAL=1
            ;;
        *)
            echo "Error: Unknown argument: ${arg}" >&2
            exit 1
            ;;
    esac
done

if [[ ! "${REPO}" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
    echo "Error: Invalid repository format '${REPO}'. Expected 'owner/repo'." >&2
    exit 1
fi

INSTALL_DIR="${HOME}/.local/bin"
BIN_PATH="${INSTALL_DIR}/${ASSET_NAME}"
ICON_BASE="${HOME}/.local/share/icons/hicolor"
ICON_DIR="${ICON_BASE}/scalable/apps"
ICON_PATH="${ICON_DIR}/${ASSET_NAME}.svg"
APP_DIR="${HOME}/.local/share/applications"
DESKTOP_PATH="${APP_DIR}/${ASSET_NAME}.desktop"

TMP_BIN="$(mktemp)"
TMP_SHA="$(mktemp)"
cleanup() {
    rm -f "${TMP_BIN}" "${TMP_SHA}"
}
trap cleanup EXIT

download_asset_with_token() {
    local token="$1"
    local asset_name="$2"
    local output_path="$3"
    local release_json="$4"

    local asset_url
    asset_url="$(printf '%s\n' "${release_json}" \
        | grep -B3 -A3 "\"name\"[[:space:]]*:[[:space:]]*\"${asset_name}\"" \
        | grep -oE 'https://api\.github\.com/repos/[^"]+/releases/assets/[0-9]+' \
        | head -n 1 || true)"

    if [[ -z "${asset_url}" ]]; then
        return 1
    fi

    curl -fsSL \
        -H "Authorization: Bearer ${token}" \
        -H "Accept: application/octet-stream" \
        "${asset_url}" \
        -o "${output_path}"
}

download_with_token() {
    local token="$1"
    local api_url="https://api.github.com/repos/${REPO}/releases/latest"
    local release_json
    release_json="$(curl -fsSL \
        -H "Authorization: Bearer ${token}" \
        -H "Accept: application/vnd.github+json" \
        "${api_url}")"

    download_asset_with_token "${token}" "${ASSET_NAME}" "${TMP_BIN}" "${release_json}" \
        && download_asset_with_token "${token}" "${CHECKSUM_ASSET}" "${TMP_SHA}" "${release_json}"
}

verify_checksum() {
    local expected_hash
    expected_hash="$(awk '{print $1}' "${TMP_SHA}" | tr '[:upper:]' '[:lower:]')"
    if [[ ! "${expected_hash}" =~ ^[0-9a-f]{64}$ ]]; then
        echo "Error: Invalid SHA-256 checksum file." >&2
        return 1
    fi

    local actual_hash
    if command -v sha256sum >/dev/null 2>&1; then
        actual_hash="$(sha256sum "${TMP_BIN}" | awk '{print $1}' | tr '[:upper:]' '[:lower:]')"
    elif command -v shasum >/dev/null 2>&1; then
        actual_hash="$(shasum -a 256 "${TMP_BIN}" | awk '{print $1}' | tr '[:upper:]' '[:lower:]')"
    else
        echo "Error: Neither sha256sum nor shasum is available to verify binary integrity." >&2
        return 1
    fi

    if [[ "${actual_hash}" != "${expected_hash}" ]]; then
        echo "Error: SHA-256 checksum mismatch for ${ASSET_NAME}." >&2
        echo "  Expected: ${expected_hash}" >&2
        echo "  Actual:   ${actual_hash}" >&2
        return 1
    fi
}

download_binary() {
    if [[ "${USE_LOCAL}" -eq 1 ]]; then
        if [[ -f "./target/release/${ASSET_NAME}" ]]; then
            echo "Using local release binary ./target/release/${ASSET_NAME}..."
            cp "./target/release/${ASSET_NAME}" "${TMP_BIN}"
            return 0
        fi
        echo "Error: Local binary ./target/release/${ASSET_NAME} not found." >&2
        return 1
    fi

    local token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

    if [[ -n "${token}" ]] && command -v curl >/dev/null 2>&1; then
        echo "Downloading ${ASSET_NAME} from GitHub release (authenticated)..."
        if download_with_token "${token}"; then
            verify_checksum
            return $?
        fi
    fi

    if command -v curl >/dev/null 2>&1; then
        local direct_url="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"
        local checksum_url="https://github.com/${REPO}/releases/latest/download/${CHECKSUM_ASSET}"
        echo "Downloading ${ASSET_NAME} from ${direct_url}..."
        if curl -fsSL "${direct_url}" -o "${TMP_BIN}" 2>/dev/null \
            && curl -fsSL "${checksum_url}" -o "${TMP_SHA}" 2>/dev/null; then
            verify_checksum
            return $?
        fi
    fi

    if command -v gh >/dev/null 2>&1; then
        echo "Downloading ${ASSET_NAME} using GitHub CLI (gh)..."
        if gh release download --repo "${REPO}" --pattern "${ASSET_NAME}" --output "${TMP_BIN}" --clobber 2>/dev/null \
            && gh release download --repo "${REPO}" --pattern "${CHECKSUM_ASSET}" --output "${TMP_SHA}" --clobber 2>/dev/null; then
            verify_checksum
            return $?
        fi
    fi

    echo "Error: Failed to download ${ASSET_NAME} and its checksum from ${REPO}." >&2
    echo "If this is a private repository, set GITHUB_TOKEN or authenticate with 'gh auth login'," >&2
    echo "or build locally with 'cargo build --release' and run './install.sh --local'." >&2
    return 1
}

ensure_path_entry() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            return 0
            ;;
    esac

    local path_line='export PATH="$HOME/.local/bin:$PATH"'
    local updated=0
    local rc_file

    for rc_file in "${HOME}/.bashrc" "${HOME}/.zshrc"; do
        if [[ -f "${rc_file}" ]]; then
            if ! grep -Fq '.local/bin' "${rc_file}"; then
                printf '\n# Added by rust-dbviewer installer\n%s\n' "${path_line}" >> "${rc_file}"
                echo "Updated ${rc_file} to include ${INSTALL_DIR} in PATH."
                updated=1
            fi
        fi
    done

    if [[ "${updated}" -eq 0 ]]; then
        echo "Note: Add ${INSTALL_DIR} to your PATH:"
        echo "  ${path_line}"
    else
        echo "Restart your shell or run 'export PATH=\"\$HOME/.local/bin:\$PATH\"' to use '${ASSET_NAME}' immediately."
    fi
}

echo "Installing Rust DBViewer..."

download_binary

if [[ ! -s "${TMP_BIN}" ]]; then
    echo "Error: Downloaded binary is empty." >&2
    exit 1
fi

mkdir -p "${INSTALL_DIR}" "${ICON_DIR}" "${APP_DIR}"

cp "${TMP_BIN}" "${BIN_PATH}"
chmod 755 "${BIN_PATH}"

cat > "${ICON_PATH}" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#242424"/>
      <stop offset="100%" stop-color="#141414"/>
    </linearGradient>
    <linearGradient id="accent" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#fad666"/>
      <stop offset="100%" stop-color="#e5a91a"/>
    </linearGradient>
  </defs>
  <rect x="6" y="6" width="116" height="116" rx="24" fill="url(#bg)" stroke="#333333" stroke-width="3"/>
  <!-- Top cylinder ellipse -->
  <ellipse cx="64" cy="36" rx="34" ry="12" fill="url(#accent)"/>
  <!-- Middle cylinder body -->
  <path d="M30 36 v20 c0 6.6 15.2 12 34 12 s34-5.4 34-12 V36 c0 6.6-15.2 12-34 12 s-34-5.4-34-12 z" fill="#f5c542" opacity="0.92"/>
  <!-- Lower cylinder body -->
  <path d="M30 61 v18 c0 6.6 15.2 12 34 12 s34-5.4 34-12 V61 c0 6.6-15.2 12-34 12 s-34-5.4-34-12 z" fill="#e5a91a" opacity="0.92"/>
  <!-- Bottom cylinder rim -->
  <path d="M30 84 v10 c0 6.6 15.2 12 34 12 s34-5.4 34-12 V84 c0 6.6-15.2 12-34 12 s-34-5.4-34-12 z" fill="#c98e12" opacity="0.92"/>
  <!-- Subtle highlight on top rim -->
  <ellipse cx="64" cy="36" rx="26" ry="7" fill="none" stroke="#fff3c4" stroke-width="2" opacity="0.55"/>
</svg>
EOF

cat > "${DESKTOP_PATH}" <<EOF
[Desktop Entry]
Type=Application
Name=Rust DBViewer
Comment=Fast native Rust MySQL and MariaDB viewer
Exec=${HOME}/.local/bin/rust-dbviewer
Icon=rust-dbviewer
Terminal=false
Categories=Development;Database;
StartupWMClass=rust-dbviewer
EOF

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APP_DIR}" >/dev/null 2>&1 || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${ICON_BASE}" >/dev/null 2>&1 || true
fi

ensure_path_entry

echo ""
echo "Rust DBViewer installed successfully!"
echo "  Binary:  ${BIN_PATH}"
echo "  Icon:    ${ICON_PATH}"
echo "  Desktop: ${DESKTOP_PATH}"

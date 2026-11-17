#!/usr/bin/env bash
# Build the app, install it to ~/Applications and put a shortcut on the Desktop.
#   ./scripts/macos-install.sh
# Requires: Rust (https://rustup.rs) and Xcode Command Line Tools (`xcode-select --install`).
set -euo pipefail

cd "$(dirname "$0")/.."
APP_NAME="Rust Clean Browser"
DEST_DIR="${HOME}/Applications"

bash scripts/macos-bundle.sh

mkdir -p "${DEST_DIR}"
rm -rf "${DEST_DIR}/${APP_NAME}.app"
cp -R "dist/${APP_NAME}.app" "${DEST_DIR}/"

# Desktop shortcut (Finder alias; falls back to a symlink if Finder automation is denied).
rm -rf "${HOME}/Desktop/${APP_NAME}.app alias" "${HOME}/Desktop/${APP_NAME}"
if ! osascript -e "tell application \"Finder\" to make alias file to POSIX file \"${DEST_DIR}/${APP_NAME}.app\" at POSIX file \"${HOME}/Desktop\"" >/dev/null 2>&1; then
  ln -sfn "${DEST_DIR}/${APP_NAME}.app" "${HOME}/Desktop/${APP_NAME}"
fi

echo "Installed: ${DEST_DIR}/${APP_NAME}.app (Desktop shortcut created)"

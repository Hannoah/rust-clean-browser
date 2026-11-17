#!/usr/bin/env bash
# Build "dist/Rust Clean Browser.app" (release build + Info.plist + icon, ad-hoc signed).
#   ./scripts/macos-bundle.sh
set -euo pipefail

cd "$(dirname "$0")/.."
APP_NAME="Rust Clean Browser"
BUNDLE="dist/${APP_NAME}.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

cargo build --release

rm -rf "${BUNDLE}"
mkdir -p "${BUNDLE}/Contents/MacOS" "${BUNDLE}/Contents/Resources"
cp target/release/rust-browser "${BUNDLE}/Contents/MacOS/rust-browser"
cp assets/icon.icns "${BUNDLE}/Contents/Resources/icon.icns"
cp LICENSE "${BUNDLE}/Contents/Resources/LICENSE.txt"

cat > "${BUNDLE}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key><string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key><string>io.github.hannoah.rust-clean-browser</string>
  <key>CFBundleExecutable</key><string>rust-browser</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>Web site URL</string>
    <key>CFBundleURLSchemes</key><array><string>http</string><string>https</string></array>
  </dict></array>
</dict>
</plist>
PLIST

# Ad-hoc signature so Gatekeeper on Apple Silicon launches the locally built app.
codesign --force --deep --sign - "${BUNDLE}" >/dev/null 2>&1 || true
echo "${BUNDLE}"

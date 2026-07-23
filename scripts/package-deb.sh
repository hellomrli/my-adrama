#!/usr/bin/env bash
# Build a simple amd64 .deb from target/release/adrama
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN="${BIN:-target/release/adrama}"
if [[ ! -x "$BIN" ]]; then
  echo "error: missing $BIN — run cargo build --release first" >&2
  exit 1
fi

NAME="adrama"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
ARCH="amd64"
OUT_DIR="${OUT:-dist}"
PKG_ROOT="$(mktemp -d)"
trap 'rm -rf "$PKG_ROOT"' EXIT

mkdir -p \
  "$PKG_ROOT/DEBIAN" \
  "$PKG_ROOT/usr/bin" \
  "$PKG_ROOT/usr/share/applications" \
  "$PKG_ROOT/usr/share/doc/${NAME}" \
  "$PKG_ROOT/usr/share/icons/hicolor/scalable/apps"

install -m 755 "$BIN" "$PKG_ROOT/usr/bin/adrama"

cat > "$PKG_ROOT/DEBIAN/control" << EOF
Package: ${NAME}
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: my-adrama contributors <noreply@users.noreply.github.com>
Depends: libxkbcommon0, libxcb1, libxcb-render0, libxcb-shape0, libxcb-xfixes0, libgtk-3-0, libayatana-appindicator3-1
Description: AI short-drama production workflow (GUI + CLI)
 Pipeline: script → parse → assets → storyboard → video.
 Opens a desktop GUI when run with no arguments.
Homepage: https://github.com/hellomrli/my-adrama
EOF

cat > "$PKG_ROOT/usr/share/applications/adrama.desktop" << 'EOF'
[Desktop Entry]
Type=Application
Name=adrama
GenericName=AI Short Drama Workflow
Comment=AI short-drama production workflow
Exec=adrama
Icon=adrama
Terminal=false
Categories=AudioVideo;Graphics;Development;
Keywords=ai;drama;video;storyboard;
EOF

cat > "$PKG_ROOT/usr/share/doc/${NAME}/copyright" << 'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: adrama
Source: https://github.com/hellomrli/my-adrama

Files: *
Copyright: my-adrama contributors
License: MIT
EOF

cp README.md "$PKG_ROOT/usr/share/doc/${NAME}/README.md" 2>/dev/null || true

# Minimal SVG icon
cat > "$PKG_ROOT/usr/share/icons/hicolor/scalable/apps/adrama.svg" << 'EOF'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
  <rect width="128" height="128" rx="24" fill="#1e1e24"/>
  <text x="64" y="78" text-anchor="middle" font-size="48" font-family="sans-serif" fill="#6ab0ff">A</text>
</svg>
EOF

mkdir -p "$OUT_DIR"
DEB="${OUT_DIR}/${NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$PKG_ROOT" "$DEB"
echo "wrote $DEB"
ls -lh "$DEB"

#!/usr/bin/env bash
# Cross-compile Windows release and pack a portable dist/ folder.
# Requires: rustup target x86_64-pc-windows-gnullvm + llvm-mingw on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET="${TARGET:-x86_64-pc-windows-gnullvm}"
OUT="${OUT:-dist}"
LLVM_MINGW_BIN="${LLVM_MINGW_BIN:-$HOME/.local/llvm-mingw/bin}"

if [[ -d "$LLVM_MINGW_BIN" ]]; then
  export PATH="$LLVM_MINGW_BIN:$PATH"
fi

if ! command -v x86_64-w64-mingw32-clang >/dev/null 2>&1; then
  echo "error: x86_64-w64-mingw32-clang not found (install llvm-mingw and put it on PATH)" >&2
  exit 1
fi

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "installing rustc target $TARGET ..."
  rustup target add "$TARGET"
fi

echo "building --release --target $TARGET ..."
cargo build --release --target "$TARGET"

EXE="target/$TARGET/release/adrama.exe"
if [[ ! -f "$EXE" ]]; then
  echo "error: missing $EXE" >&2
  exit 1
fi

# Fail if libunwind.dll is still dynamically linked
if command -v x86_64-w64-mingw32-objdump >/dev/null 2>&1; then
  deps="$(x86_64-w64-mingw32-objdump -p "$EXE" | grep -i 'DLL Name' || true)"
  if echo "$deps" | grep -qi 'libunwind\.dll'; then
    echo "error: binary still depends on libunwind.dll; check .cargo/config.toml rustflags" >&2
    echo "$deps"
    exit 1
  fi
  echo "DLL imports:"
  echo "$deps"
fi

rm -rf "$OUT"
mkdir -p "$OUT"
cp "$EXE" "$OUT/adrama.exe"

# Optional: ship runtime DLLs next to the exe (not required when statically linked).
RUNTIME_DIR="${LLVM_MINGW_BIN%/bin}/x86_64-w64-mingw32/bin"
if [[ -d "$RUNTIME_DIR" ]]; then
  for dll in libunwind.dll libwinpthread-1.dll; do
    if [[ -f "$RUNTIME_DIR/$dll" ]]; then
      cp "$RUNTIME_DIR/$dll" "$OUT/"
    fi
  done
fi

cat > "$OUT/README.txt" << 'EOF'
adrama - AI short-drama production workflow (GUI + CLI, Windows)

GUI (double-click or):
  adrama.exe
  adrama.exe gui
  adrama.exe --gui --project C:\path\to\my-drama

CLI:
  adrama.exe --help
  adrama.exe new my-drama
  adrama.exe parse

Environment (or set in GUI → Settings for this session):
  OPENAI_API_KEY=...
  GEMINI_API_KEY=...

Optional:
  ffmpeg on PATH for video --concat

If Windows reports a missing DLL, keep the .dll files from this folder
next to adrama.exe (same directory).
EOF

# Portable zip when zip is available
if command -v zip >/dev/null 2>&1; then
  ZIP="adrama-windows-x86_64.zip"
  rm -f "$ZIP"
  (cd "$OUT" && zip -9 "../$ZIP" ./*)
  echo "wrote $ZIP"
fi

echo "packed -> $OUT/"
ls -la "$OUT"

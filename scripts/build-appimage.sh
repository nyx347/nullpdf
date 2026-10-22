#!/usr/bin/env bash
#
# Build a nullpdf AppImage (x86_64) into ./dist, bundling libpdfium.
#
#   scripts/build-appimage.sh <pdfium-dir>
#
# <pdfium-dir> is an unpacked pdfium-binaries release (lib/libpdfium.so, LICENSE,
# licenses/), e.g. pdfium-linux-x64.tgz from github.com/bblanchon/pdfium-binaries; use the
# chromium/NNNN release matching pdfium-render's `pdfium_latest`. appimagetool must be on
# PATH (or named by $APPIMAGETOOL). The AppImage runs on glibc as old as the build host's.

set -euo pipefail

if [[ $# -ne 1 || ! -f "$1/lib/libpdfium.so" ]]; then
	echo "usage: ${0##*/} <pdfium-dir containing lib/libpdfium.so>" >&2
	exit 2
fi
pdfium="$(cd "$1" && pwd)"
appimagetool="${APPIMAGETOOL:-appimagetool}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
version="$(awk -F'"' '/^version *=/ {print $2; exit}' Cargo.toml)"

cargo build --release --locked

appdir=dist/AppDir
rm -rf "$appdir"
install -Dm755 target/release/nullpdf "$appdir/usr/bin/nullpdf"
strip "$appdir/usr/bin/nullpdf"
# Found at runtime as ../lib/libpdfium.so next to the executable (PdfDoc::init_pdfium).
install -Dm644 "$pdfium/lib/libpdfium.so" "$appdir/usr/lib/libpdfium.so"
install -Dm644 "$pdfium/LICENSE" "$appdir/usr/share/doc/pdfium/LICENSE"
cp -r "$pdfium/licenses" "$appdir/usr/share/doc/pdfium/"
install -Dm644 packaging/nullpdf.desktop "$appdir/usr/share/applications/nullpdf.desktop"
install -Dm644 packaging/nullpdf.png "$appdir/usr/share/icons/hicolor/256x256/apps/nullpdf.png"
cp packaging/nullpdf.desktop packaging/nullpdf.png "$appdir/"
ln -s nullpdf.png "$appdir/.DirIcon"
ln -s usr/bin/nullpdf "$appdir/AppRun"

out="dist/nullpdf-$version-x86_64.AppImage"
ARCH=x86_64 "$appimagetool" --no-appstream "$appdir" "$out"
echo
echo "Built: $out"

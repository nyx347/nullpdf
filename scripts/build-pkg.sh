#!/usr/bin/env bash
#
# Build a nullpdf Arch package (.pkg.tar.zst) from the current working tree.
#
#   scripts/build-pkg.sh             build the package into ./dist
#   scripts/build-pkg.sh --install   build, then install it with `sudo pacman -U`
#
# The finished package is left in ./dist. makepkg's scratch dirs (src/, pkg/) are
# redirected into ./dist/build so the repo root stays clean.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

install_pkg=0
case "${1:-}" in
	--install) install_pkg=1 ;;
	"") ;;
	*) echo "usage: ${0##*/} [--install]" >&2; exit 2 ;;
esac

mkdir -p dist/build

# PKGDEST: where the .pkg.tar.zst lands. BUILDDIR: where src/ and pkg/ go.
PKGDEST="$repo_root/dist" BUILDDIR="$repo_root/dist/build" makepkg -f --noconfirm

pkg="$(ls -t "$repo_root"/dist/nullpdf-*.pkg.tar.zst | head -1)"
echo
echo "Built: $pkg"

if [[ "$install_pkg" == 1 ]]; then
	echo "Installing with pacman -U ..."
	sudo pacman -U --noconfirm "$pkg"
else
	echo "Install with: sudo pacman -U \"$pkg\""
fi

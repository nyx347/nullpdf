# Maintainer: Nicholas Cluff <nyx@darkcloud.app>
#
# Builds the nullpdf binary from the current working tree (the directory this
# PKGBUILD lives in). Use scripts/build-pkg.sh, or run `makepkg -f` here directly.

pkgname=nullpdf
pkgver=0.2.0
pkgrel=1
pkgdesc='Minimal PDF reader focused on freehand drawing and text annotation'
arch=('x86_64')
url='https://github.com/NicholasCluff/nullpdf'
license=('custom')
# pdfium-binaries provides /usr/lib/libpdfium.so, loaded at runtime via
# Pdfium::bind_to_system_library(). The egui/winit/glow stack pulls in the rest.
depends=('pdfium-binaries' 'gcc-libs' 'glibc' 'fontconfig' 'libxkbcommon' 'libglvnd')
# cargo must be on PATH at build time (the 'rust'/'cargo' package, or rustup).
# Not listed in makedepends so a rustup-managed toolchain isn't flagged as missing.
makedepends=()
options=('!lto')

pkgver() {
	# Keep the package version in sync with Cargo.toml.
	awk -F'"' '/^version *=/ {print $2; exit}' "$startdir/Cargo.toml"
}

prepare() {
	cd "$startdir"
	export CARGO_HOME="$startdir/target/cargo-home"
	cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
	cd "$startdir"
	export CARGO_HOME="$startdir/target/cargo-home"
	export RUSTUP_TOOLCHAIN=stable
	cargo build --frozen --release --all-features
}

package() {
	cd "$startdir"
	install -Dm755 "target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
}

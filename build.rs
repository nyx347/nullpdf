//! Embeds the app icon and version info in the Windows exe, for Explorer, the taskbar and
//! "Open with". The icon comes from `packaging/render-icon.py`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=packaging/nullpdf.ico");
    // A build script is compiled for the host: ask Cargo what the target is.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("packaging/nullpdf.ico")
            .compile_for("nullpdf")
            .expect("compiling the Windows resources (rc.exe; llvm-rc to cross-compile)");
    }
}

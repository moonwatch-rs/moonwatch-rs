//! Embed the Windows resources - the application icon and the version information block -
//! into the executable. Everything here is a no-op for any other target.
//!
//! Windows takes an executable's icon from a resource linked into the PE file, which is why
//! the `.png` the tray uses is not enough by itself: `share/moonwatch-icon.ico` is generated
//! alongside it by `share/make_icons.py` and checked in.

fn main() {
    println!("cargo:rerun-if-changed=share/moonwatch-icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // The *target* OS, not the host. Windows binaries are also cross-compiled from Linux
    // (see build_windows.py), which is likewise why `winresource` is an unconditional
    // build-dependency: the `cfg` of a build-dependency is resolved for the host, so a
    // `cfg(windows)` one would go missing in exactly that case.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resources = winresource::WindowsResource::new();
    resources.set_icon("share/moonwatch-icon.ico");

    // What Explorer shows under Properties > Details. Both default to the Cargo package
    // name, `moonwatch-rs`, which is the name of the crate rather than of the program.
    resources.set("ProductName", "Moonwatch.rs");
    resources.set("FileDescription", env!("CARGO_PKG_DESCRIPTION"));

    // Deliberately fatal: a binary that silently lost its icon looks like a bad build, and
    // the resource compiler is part of both toolchains we build Windows with anyway.
    resources.compile().expect(
        "could not compile the Windows resources - this needs a resource compiler: rc.exe \
         from the Windows SDK for the MSVC target, or windres (mingw-w64 binutils) when \
         cross-compiling the GNU target",
    );
}

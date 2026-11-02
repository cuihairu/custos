//! Windows build: embed the application icon (assets/icons/custos.ico) into
//! the `custos-tray` executable so Explorer/taskbar/shortcuts show the brand
//! mark instead of the generic exe icon. No-op on non-Windows targets.

fn main() {
    // CARGO_CFG_TARGET_OS reflects the *target* (set by cargo for build
    // scripts), so this stays a no-op when building for Linux/macOS.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../assets/icons/custos.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().expect("icon path is valid UTF-8"));
    if let Err(err) = res.compile() {
        // Windows hosts (CI release builds) must never silently drop the
        // icon: rc.exe is guaranteed by the MSVC toolchain there. Cross
        // builds (e.g. `cargo check --target x86_64-pc-windows-*` from
        // Linux) may lack a resource compiler (rc.exe/llvm-rc/windres) —
        // warn instead of breaking the check.
        if std::env::consts::FAMILY == "windows" {
            panic!("failed to embed Windows icon into custos-tray.exe: {err}");
        } else {
            println!(
                "cargo:warning=custos-tray: Windows icon not embedded (cross build, no resource compiler): {err}"
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    // Embed icon and version info into the Windows executable
    let mut res = winres::WindowsResource::new();
    res.set("FileDescription", "Mouser terminal emulator");
    res.set("ProductName", "Mouser");
    res.set("CompanyName", "Mouser");
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    // Optional icon file; if missing, build continues
    let icon_path = "resources/windows/mouser.ico";
    if std::path::Path::new(icon_path).exists() {
        res.set_icon(icon_path);
    }
    let _ = res.compile();
}

#[cfg(not(target_os = "windows"))]
fn main() {}

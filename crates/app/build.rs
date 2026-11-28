//! Windows: puts the app's icon and version information into `pistachio.exe`, so Explorer,
//! the taskbar, the window's title bar and Task Manager show Pistachio (GPUI loads the window
//! icon from the executable's icon resource 1).

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/app-icon/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    let icon = manifest.join("assets").join("app-icon").join("icon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets it");
    let rc = out.join("pistachio.rc");
    std::fs::write(&rc, resource_script(&icon.to_string_lossy(), &version))
        .expect("writing the resource script");
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .expect("compiling the Windows resources");
}

/// The resource script: icon 1 and a version block.
fn resource_script(icon: &str, version: &str) -> String {
    let numbers: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    let (major, minor, patch) = (
        numbers.first().copied().unwrap_or(0),
        numbers.get(1).copied().unwrap_or(0),
        numbers.get(2).copied().unwrap_or(0),
    );
    let icon = icon.replace('\\', "\\\\");
    format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Pistachio"
            VALUE "FileDescription", "Pistachio"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "pistachio"
            VALUE "LegalCopyright", "MIT OR Apache-2.0"
            VALUE "OriginalFilename", "pistachio.exe"
            VALUE "ProductName", "Pistachio"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}

#![allow(
    clippy::disallowed_methods,
    reason = "build helper used only from build scripts"
)]
#![cfg(target_os = "windows")]

fn product_version() -> String {
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    if cfg!(debug_assertions) {
        format!("{pkg_version}-dev")
    } else {
        format!("{pkg_version}")
    }
}

const ICON_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

pub fn compile() -> Result<(), Box<dyn std::error::Error>> {
    let product_name = std::env::var("CARGO_PKG_NAME").unwrap();
    let icon = std::path::PathBuf::from(ICON_DIR).join("app-icon.ico");
    let icon_escaped = icon.to_string_lossy().replace('\\', "\\\\");

    let manifest_line = String::new();

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let product_version = product_version();
    let mut version_parts = pkg_version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0));
    let file_version = format!(
        "{},{},{},{}",
        version_parts.next().unwrap_or(0),
        version_parts.next().unwrap_or(0),
        version_parts.next().unwrap_or(0),
        version_parts.next().unwrap_or(0),
    );

    let rc_content = format!(
        r#"1 ICON "{icon_escaped}"
{manifest_line}

1 VERSIONINFO
FILEVERSION {file_version}
PRODUCTVERSION {file_version}
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
            VALUE "FileDescription", "{product_name}\0"
            VALUE "FileVersion", "{pkg_version}\0"
            VALUE "ProductName", "{product_name}\0"
            VALUE "ProductVersion", "{product_version}\0"
            VALUE "CompanyName", "zeal\0"
            VALUE "LegalCopyright", "-\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
    );

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let rc_path = out_dir.join("alacrterm_resources.rc");
    std::fs::write(&rc_path, rc_content)?;

    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .unwrap();

    Ok(())
}

fn main() {
    crate::compile().expect("failed to compile Windows resources");
}

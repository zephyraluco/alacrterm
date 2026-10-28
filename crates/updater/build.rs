fn main() {
    println!("cargo:rustc-check-cfg=cfg(sparkle)");
    println!("cargo:rerun-if-env-changed=SPARKLE_FRAMEWORK_DIR");
    println!("cargo:rerun-if-changed=native/sparkle.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Some(directory) = std::env::var_os("SPARKLE_FRAMEWORK_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    assert!(
        directory.join("Sparkle.framework").is_dir(),
        "SPARKLE_FRAMEWORK_DIR must contain Sparkle.framework"
    );
    cc::Build::new()
        .file("native/sparkle.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag(format!("-F{}", directory.display()))
        .compile("alacrterm_sparkle");
    println!("cargo:rustc-cfg=sparkle");
    println!("cargo:rustc-link-search=framework={}", directory.display());
    println!("cargo:rustc-link-lib=framework=Sparkle");
    println!("cargo:rustc-link-lib=framework=AppKit");
}

fn main() {
    println!("cargo:rerun-if-changed=native/exit.m");
    println!("cargo:rerun-if-changed=../../assets/app-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../assets/app-icon.ico");
        assert!(icon.is_file(), "application icon is missing");
        let resource =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("app-icon.rc");
        // GPUI loads the application window icon from Windows resource ID 1.
        let path = icon.to_string_lossy().replace('\\', "/");
        std::fs::write(&resource, format!("1 ICON \"{path}\"\n"))
            .expect("failed to write icon resource");
        embed_resource::compile_for(&resource, ["alacrterm"], embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed application icon");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("native/exit.m")
            .flag("-fobjc-arc")
            .compile("alacrterm_exit");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }
}

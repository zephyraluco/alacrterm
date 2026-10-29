fn main() {
    println!("cargo:rerun-if-changed=native/exit.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("native/exit.m")
            .flag("-fobjc-arc")
            .compile("alacrterm_exit");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }
}

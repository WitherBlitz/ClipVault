fn main() {
    println!("cargo:rerun-if-changed=assets/clipvault.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    if std::path::Path::new("assets/clipvault.ico").exists() {
        winresource::WindowsResource::new()
            .set_icon("assets/clipvault.ico")
            .set_manifest_file("assets/app.manifest")
            .set("ProductName", "ClipVault")
            .set("FileDescription", "ClipVault Clipboard History")
            .compile()
            .expect("compile Windows resources");
    }
}

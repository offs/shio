fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/icon.ico");
    println!("cargo:rerun-if-changed=../../assets/windows/shio.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Windows shows FileDescription as the app name in Task Manager and dialogs.
    winresource::WindowsResource::new()
        .set_icon("../../assets/icons/icon.ico")
        .set_manifest_file("../../assets/windows/shio.manifest")
        .set("ProductName", "shio")
        .set("FileDescription", "shio")
        .set("OriginalFilename", "shio.exe")
        .compile()
        .expect("Windows resources compile");
}

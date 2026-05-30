fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icons/icon.ico");
        res.set_manifest_file("../../assets/windows/shio.manifest");
        res.set("CompanyName", "shio");
        res.set("ProductName", "shio");
        res.set("FileDescription", "download manager");
        res.set("OriginalFilename", "shio.exe");
        res.set("InternalName", "shio");
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.compile()
            .unwrap_or_else(|e| panic!("winresource compile failed: {e}"));
    }
    println!("cargo:rerun-if-changed=../../assets/icons/icon.ico");
    println!("cargo:rerun-if-changed=../../assets/windows/shio.manifest");
}

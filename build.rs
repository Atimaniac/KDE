// Embed the Delfin icon in delfin.exe (Explorer, shortcuts, Settings › Apps).
fn main() {
    println!("cargo:rerun-if-changed=assets/delfin.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/delfin.ico");
        res.set("FileDescription", "Delfin");
        res.set("ProductName", "Delfin");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}

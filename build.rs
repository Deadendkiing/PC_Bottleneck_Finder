// Embeds the icon and version information into the Windows .exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "PC Bottleneck Finder")
            .set("FileDescription", "PC Bottleneck Finder")
            .set("CompanyName", "Deadendkiing");
        res.compile().expect("failed to embed Windows resources");
    }
}

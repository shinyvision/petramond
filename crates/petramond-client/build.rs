fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/petramond.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("../../packaging/icons/petramond.ico")
        .set("ProductName", "Petramond")
        .set("FileDescription", "Petramond");
    resource
        .compile()
        .expect("embedding the Windows icon resource");
}

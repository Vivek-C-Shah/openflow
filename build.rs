//! Embeds the app icon and version info into the Windows executables.
fn main() {
    println!("cargo:rerun-if-changed=assets/openflow.rc");
    println!("cargo:rerun-if-changed=assets/openflow.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/openflow.rc", embed_resource::NONE);
    }
}

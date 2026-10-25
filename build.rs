//! Windows executable icon. Other targets need no resources.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // rc.exe/windres need forward slashes in paths.
    let icon = manifest_dir.join("assets").join("icon.ico").display().to_string().replace('\\', "/");
    let rc = out_dir.join("icon.rc");
    std::fs::write(&rc, format!("1 ICON \"{icon}\"\n")).expect("write resource script");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("compile Windows icon");
}

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir.parent().expect("workspace root");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("target arch");

    let linker_script = workspace_root
        .join("aether-frame")
        .join("src")
        .join("arch")
        .join(&target_arch)
        .join("linker.ld");

    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rustc-link-arg=-no-pie");
}

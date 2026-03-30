fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let linker = format!("linker-{arch}.ld");
    println!("cargo:rustc-link-arg=-T{linker}");
    println!("cargo:rustc-link-search={manifest_dir}");
    println!("cargo:rerun-if-changed={linker}");
}

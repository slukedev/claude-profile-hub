fn main() {
    // The updater downloads the build for the same target this binary was built for.
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=PROFILE_HUB_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}

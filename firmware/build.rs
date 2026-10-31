// Relink when the linker script changes (cargo does not track -T files).
// OPENDM40_VERSION (src/ident.rs) = the package version + DM40_VERSION_SUFFIX
// from the environment: CI sets it for a pull request build, e.g.
// "-pr148.1a2b3c4" (README "PR builds"); unset, the version is Cargo.toml's.
fn main() {
    println!("cargo:rerun-if-changed=link-native.x");
    println!("cargo:rerun-if-env-changed=DM40_VERSION_SUFFIX");
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap();
    let suffix = std::env::var("DM40_VERSION_SUFFIX").unwrap_or_default();
    // Letters, digits, '-' and '.' only: it is part of a file name too.
    assert!(
        suffix.len() <= 16
            && suffix.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.'),
        "DM40_VERSION_SUFFIX {suffix:?}: at most 16 of [A-Za-z0-9.-]"
    );
    println!("cargo:rustc-env=OPENDM40_VERSION={pkg}{suffix}");
}

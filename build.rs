//! One link flag, for macOS only.
//!
//! The `2:` entry points call into the q host that loaded us: `ka`, `krr`, `ktn`, `ss`, `xD`
//! live in the q executable, not in any library we can link against. A Linux shared object
//! may carry undefined symbols and have them resolved at dlopen; the macOS linker resolves
//! everything up front and fails with "symbol(s) not found" unless it is told to defer.
//!
//! Scoped to the cdylib, so a Rust host linking the rlib (with `kapi` off, hence no such
//! references) is unaffected, and so is every Linux build.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-cdylib=-Wl,-undefined,dynamic_lookup");
    }
}

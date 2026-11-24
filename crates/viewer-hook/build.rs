//! The library goes into another program (Moonlight), so it brings its own C runtime support
//! (vcruntime) rather than needing VCRUNTIME140.dll there. The Universal CRT stays dynamic: it is
//! part of Windows 10 and later. These link arguments apply to this crate's library only.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for lib in ["vcruntime.lib", "vcruntimed.lib", "libvcruntimed.lib"] {
            println!("cargo:rustc-link-arg=/NODEFAULTLIB:{lib}");
        }
        println!("cargo:rustc-link-arg=libvcruntime.lib");
    }
}

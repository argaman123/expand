//! The window backdrop, as the front end wants it.

/// "mica" on Windows 11 (build 22000 or later), else "solid".
pub fn backdrop() -> &'static str {
    match windows_build() {
        Some(build) if build >= 22000 => "mica",
        _ => "solid",
    }
}

#[cfg(windows)]
fn windows_build() -> Option<u32> {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo { size: std::mem::size_of::<OsVersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
    // SAFETY: `info` is a valid, correctly sized OSVERSIONINFOW.
    (unsafe { RtlGetVersion(&mut info) } == 0).then_some(info.build)
}

#[cfg(not(windows))]
fn windows_build() -> Option<u32> {
    None
}

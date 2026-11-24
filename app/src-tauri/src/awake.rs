//! Keeps the Mac from sleeping while a PC is connected: a power assertion held for as long as it
//! is wanted and released when it is not.

#[cfg(target_os = "macos")]
mod iokit {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use std::sync::Mutex;

    type AssertionId = u32;
    /// `kIOPMAssertionLevelOn`.
    const LEVEL_ON: u32 = 255;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut AssertionId) -> i32;
        fn IOPMAssertionRelease(id: AssertionId) -> i32;
    }

    static HELD: Mutex<Option<AssertionId>> = Mutex::new(None);

    /// Holds or releases the assertion so that it matches `wanted`. Asking twice changes nothing.
    pub fn set(wanted: impl FnOnce() -> bool) {
        let mut held = HELD.lock().unwrap();
        // Decided under the lock: a slow caller cannot put back what a newer one released.
        match (wanted(), *held) {
            (true, None) => {
                // kIOPMAssertionTypePreventUserIdleSystemSleep: the screen may sleep, the Mac not.
                let kind = CFString::new("PreventUserIdleSystemSleep");
                let reason = CFString::new("Sidescreen connected");
                let mut id: AssertionId = 0;
                let status = unsafe {
                    IOPMAssertionCreateWithName(kind.as_concrete_TypeRef(), LEVEL_ON, reason.as_concrete_TypeRef(), &mut id)
                };
                if status == 0 {
                    *held = Some(id);
                }
            }
            (false, Some(id)) => {
                unsafe { IOPMAssertionRelease(id) };
                *held = None;
            }
            _ => {}
        }
    }
}

/// Makes the Mac stay awake (or not), as `wanted` says when it is asked. Does nothing on the PC.
pub fn set(wanted: impl FnOnce() -> bool) {
    #[cfg(target_os = "macos")]
    iokit::set(wanted);
    #[cfg(not(target_os = "macos"))]
    let _ = wanted;
}

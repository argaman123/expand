//! The Mac's event tap for Ctrl as ⌘ in the display window: a thin layer over
//! [`super::StreamKeys`]. macOS only, and not built on Windows: every line here is unbuilt there.
//!
//! The tap sits at the session tap, at the head, as an active filter, on its own thread and run
//! loop. Sunshine posts its keys at `kCGSessionEventTap` and its mouse at `kCGHIDEventTap`, and
//! Apple's `CGEvent.h` says an event posted at a location "passes through any such taps", so
//! both come by here. It looks only at events whose source pid (`kCGEventSourceUnixProcessID`)
//! is the Sunshine this app started; everything else (the user's own keyboard, this app's
//! mouse-sharing input, every other app) goes on untouched. If that field never carries
//! Sunshine's pid, nothing matches and nothing is rewritten: the stream types as before.
//!
//! The tap uses core-graphics 0.25 (the crate's 0.24 wrapper cannot drop an event): the
//! callback changes an event in place and keeps it, or drops it (returns NULL, as `CGEventTypes.h`
//! says deletes it) after keeping a copy, which is posted from the tap when its turn comes.

use super::{Event, Kind, StreamKeys, This};
use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult};
use core_graphics_25::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
    CGEventType, CallbackResult, EventField,
};
use foreign_types::ForeignType;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventCreateCopy(event: core_graphics_25::sys::CGEventRef) -> core_graphics_25::sys::CGEventRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// How long the tap's run loop sleeps between looks at the stop flag, in case a stop came just
/// before it started running. The tap is inert from the stop on, whatever this is.
const LOOK_AGAIN: Duration = Duration::from_secs(10);

/// A running tap. Dropping it stops it: from that moment its callback lets everything through,
/// and its thread removes it shortly after.
pub struct Tap {
    stop: Arc<AtomicBool>,
    run_loop: CFRunLoop,
}

impl Tap {
    /// Taps the keys Sunshine (`sunshine`, its pid) posts. `None` when the tap could not be made:
    /// then nothing is rewritten.
    pub fn start(sunshine: u32) -> Option<Tap> {
        // Without Accessibility the system would quietly leave the keys out of the tap, and only
        // the mouse's flags would change.
        if !unsafe { AXIsProcessTrusted() } {
            crate::sharelog::line("stream keys: no Accessibility permission, Ctrl stays Control in the display");
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("stream-keys".into())
            .spawn(move || run(sunshine, flag, ready_tx))
            .ok()?;
        let run_loop = ready_rx.recv().ok().flatten()?;
        crate::sharelog::line(format!("stream keys: tapping Sunshine (pid {sunshine})"));
        Some(Tap { stop, run_loop })
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.run_loop.stop();
    }
}

fn kind_of(etype: CGEventType) -> Option<Kind> {
    match etype {
        CGEventType::KeyDown => Some(Kind::KeyDown),
        CGEventType::KeyUp => Some(Kind::KeyUp),
        CGEventType::FlagsChanged => Some(Kind::FlagsChanged),
        CGEventType::LeftMouseDown
        | CGEventType::LeftMouseUp
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDown
        | CGEventType::RightMouseUp
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDown
        | CGEventType::OtherMouseUp
        | CGEventType::OtherMouseDragged
        | CGEventType::ScrollWheel => Some(Kind::Pointer),
        _ => None,
    }
}

/// Writes `keycode` (key events only, when it differs) and `flags` into `event`.
fn write(event: &CGEvent, kind: Kind, keycode: u16, flags: u64) {
    if kind != Kind::Pointer && event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) != i64::from(keycode) {
        event.set_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE, i64::from(keycode));
    }
    event.set_flags(CGEventFlags::from_bits_retain(flags));
}

/// The tap's thread: makes the tap, says whether it could, and runs it until told to stop.
fn run(sunshine: u32, stop: Arc<AtomicBool>, ready: std::sync::mpsc::Sender<Option<CFRunLoop>>) {
    let keys = RefCell::new(StreamKeys::default());
    let held: RefCell<Option<CGEvent>> = RefCell::new(None);
    let port: Cell<CFMachPortRef> = Cell::new(std::ptr::null_mut());
    let rewritten = Cell::new(0u64);
    let callback = |proxy: CGEventTapProxy, etype: CGEventType, event: &CGEvent| -> CallbackResult {
        if matches!(etype, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput) {
            // The system turned the tap off (it was slow, or the user's input did): Sunshine's
            // events went by unseen meanwhile, so start over with nothing held (a held Control
            // posted later could leave ⌘ down), then back on.
            *keys.borrow_mut() = StreamKeys::default();
            held.borrow_mut().take();
            if !stop.load(Ordering::Relaxed) && !port.get().is_null() {
                unsafe { CGEventTapEnable(port.get(), true) };
            }
            return CallbackResult::Keep;
        }
        if stop.load(Ordering::Relaxed)
            || event.get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID) != i64::from(sunshine)
        {
            return CallbackResult::Keep;
        }
        let Some(kind) = kind_of(etype) else { return CallbackResult::Keep };
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        let flags = event.get_flags().bits();
        let step = keys.borrow_mut().event(Event { kind, keycode, flags }, Instant::now());
        if let Some(change) = step.release_held {
            let copy = held.borrow_mut().take();
            if let Some(copy) = copy {
                write(&copy, Kind::FlagsChanged, change.keycode, change.flags);
                copy.post_from_tap(proxy);
            }
        }
        match step.this {
            This::Pass(change) => {
                if (change.keycode, change.flags) != (keycode, flags) {
                    write(event, kind, change.keycode, change.flags);
                    rewritten.set(rewritten.get() + 1);
                }
                CallbackResult::Keep
            }
            This::Hold => {
                let copy = unsafe { CGEventCreateCopy(event.as_ptr()) };
                // Without a copy it cannot be held back: it goes now, as Command.
                if copy.is_null() {
                    if let Some(change) = keys.borrow_mut().cannot_hold() {
                        write(event, kind, change.keycode, change.flags);
                    }
                    return CallbackResult::Keep;
                }
                *held.borrow_mut() = Some(unsafe { CGEvent::from_ptr(copy) });
                CallbackResult::Drop
            }
            This::Drop => CallbackResult::Drop,
        }
    };
    let mask = vec![
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
        CGEventType::ScrollWheel,
    ];
    // Drags come only while a button is down; plain moves stay out, to keep the tap light.
    // SAFETY: the tap is declared after everything its callback borrows, so it is dropped (and
    // its port invalidated) first; and it is only added to this thread's run loop.
    let tap = unsafe {
        CGEventTap::new_unchecked(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            mask,
            callback,
        )
    };
    let Ok(tap) = tap else {
        crate::sharelog::line("stream keys: the keyboard could not be tapped");
        let _ = ready.send(None);
        return;
    };
    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        drop(tap);
        crate::sharelog::line("stream keys: the tap has no run loop source");
        let _ = ready.send(None);
        return;
    };
    port.set(tap.mach_port().as_concrete_TypeRef());
    let run_loop = CFRunLoop::get_current();
    run_loop.add_source(&source, unsafe { kCFRunLoopDefaultMode });
    tap.enable();
    if ready.send(Some(run_loop)).is_err() {
        return;
    }
    let mut ended_by_system = false;
    while !stop.load(Ordering::SeqCst) {
        // `Finished`: the run loop has no source left (the system invalidated the tap's port).
        // Running it again would return at once, over and over.
        if CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, LOOK_AGAIN, false) == CFRunLoopRunResult::Finished {
            ended_by_system = true;
            break;
        }
    }
    stop.store(true, Ordering::SeqCst);
    // Removed before anything else: dropping the tap invalidates and releases its port, which
    // removes it (`CGEvent.h`), so no event waits on a run loop nobody runs any more.
    drop(source);
    drop(tap);
    let why = if ended_by_system { "ended by the system" } else { "removed" };
    crate::sharelog::line(format!("stream keys: tap {why}, {} of Sunshine's events rewritten", rewritten.get()));
}

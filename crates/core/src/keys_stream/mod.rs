//! Ctrl as ⌘ in the display window: what the Mac does to the keys Sunshine types for the PC.
//!
//! Typing into the stream goes Moonlight → Sunshine, and Sunshine types the PC's Ctrl as
//! Control on the Mac (and the Windows key, which Moonlight does not send, as Command). The PC
//! cannot change that on its way out, so while the PC's setting is on and a display session
//! runs, the Mac taps its own keyboard events and rewrites only the ones Sunshine posted
//! (`tap`, macOS only): Control becomes Command. One way only, unlike the sharing path
//! (`share::keys`): Moonlight does not send Win, so Sunshine's Command is the Menu key's, and
//! it stays Command.
//!
//! What is rewritten, from Sunshine's macOS input (libvirtualhid's macOS backend): key down
//! and up events carry Sunshine's modifier flags, a modifier key is a FlagsChanged event with
//! its key code, and mouse buttons and scrolls carry the same flags. So: the flags' Control
//! bits (the device-independent one and the left/right ones) become Command's on every
//! event, and FlagsChanged key codes go from Control to Command (left 0x3B → 0x37, right 0x3E →
//! 0x36). Every other bit and key stays as it is.
//!
//! AltGr is not Ctrl: Windows sends it as a left Ctrl going down just before the right Alt, and
//! Moonlight forwards both. So a left Control going down is held back until Sunshine's next
//! event. With a right Option going down right after it (within [`ALTGR_WINDOW`]) both go as
//! they are, Control and Option, and keep going so until that Control is let go; anything else
//! means Control was Control, and it goes as Command first. This mirrors `share::keys`.

use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod tap;

/// macOS virtual key codes (HIToolbox's `Events.h`).
pub mod mac_key {
    pub const COMMAND: u16 = 0x37;
    pub const RIGHT_COMMAND: u16 = 0x36;
    pub const CONTROL: u16 = 0x3B;
    pub const RIGHT_CONTROL: u16 = 0x3E;
    pub const RIGHT_OPTION: u16 = 0x3D;
}

/// Event flag bits (IOKit's `IOLLEvent.h`): the device-independent ones, as `CGEventFlags` has
/// them, and the left/right ones a keyboard (and Sunshine) sets beside them.
pub mod flag {
    pub const CONTROL: u64 = 0x0004_0000;
    pub const ALTERNATE: u64 = 0x0008_0000;
    pub const COMMAND: u64 = 0x0010_0000;
    pub const DEVICE_LEFT_CONTROL: u64 = 0x0000_0001;
    pub const DEVICE_RIGHT_CONTROL: u64 = 0x0000_2000;
    pub const DEVICE_LEFT_COMMAND: u64 = 0x0000_0008;
    pub const DEVICE_RIGHT_COMMAND: u64 = 0x0000_0010;
    pub const DEVICE_LEFT_ALTERNATE: u64 = 0x0000_0020;
    pub const DEVICE_RIGHT_ALTERNATE: u64 = 0x0000_0040;
}

/// How soon after a left Control the right Option must go down for the two to be AltGr.
/// Windows makes both at once and Moonlight sends them back to back; a person pressing Control
/// and then right Option on purpose is far slower.
pub const ALTGR_WINDOW: Duration = Duration::from_millis(50);

/// The kinds of Sunshine's events the tap looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    KeyDown,
    KeyUp,
    /// A modifier key went down or up; its key code says which.
    FlagsChanged,
    /// A mouse button or a scroll: only its flags count.
    Pointer,
}

/// One of Sunshine's events, as the tap reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    /// Meaningful for key events only.
    pub keycode: u16,
    pub flags: u64,
}

/// The key code and flags an event is to carry (for a pointer event, only the flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub keycode: u16,
    pub flags: u64,
}

/// What becomes of the event itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum This {
    /// It goes on, carrying these (which may be what it had).
    Pass(Change),
    /// A left Control held back to see whether it is AltGr's: keep a copy, and let nothing go.
    Hold,
    /// A repeat of the Control held back: let nothing go.
    Drop,
}

/// What to do for one of Sunshine's events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The Control held back earlier goes first, carrying these.
    pub release_held: Option<Change>,
    pub this: This,
}

/// The flags with Control as Command (both the device-independent and the left/right bits);
/// every other bit as it is, Command's own included. One way only: Sunshine's Command comes
/// from the Menu key (Moonlight does not send Win), and it stays Command.
pub fn command_flags(flags: u64) -> u64 {
    let pairs = [
        (flag::CONTROL, flag::COMMAND),
        (flag::DEVICE_LEFT_CONTROL, flag::DEVICE_LEFT_COMMAND),
        (flag::DEVICE_RIGHT_CONTROL, flag::DEVICE_RIGHT_COMMAND),
    ];
    pairs.iter().fold(flags, |out, &(control, command)| {
        if flags & control != 0 {
            (out & !control) | command
        } else {
            out
        }
    })
}

/// A modifier's key code with Control as Command; every other key (Command included) as it is.
pub fn command_key(code: u16) -> u16 {
    match code {
        mac_key::CONTROL => mac_key::COMMAND,
        mac_key::RIGHT_CONTROL => mac_key::RIGHT_COMMAND,
        other => other,
    }
}

/// Whether this FlagsChanged event has left/right Control bits to tell the two apart by.
fn has_control_sides(flags: u64) -> bool {
    flags & (flag::DEVICE_LEFT_CONTROL | flag::DEVICE_RIGHT_CONTROL) != 0
}

/// Whether the left Control is down after a FlagsChanged event with these flags: its own bit
/// when the event has left/right bits, else the device-independent one.
fn left_control_down(flags: u64) -> bool {
    if has_control_sides(flags) {
        flags & flag::DEVICE_LEFT_CONTROL != 0
    } else {
        flags & flag::CONTROL != 0
    }
}

/// Whether the right Option is down after a FlagsChanged event with these flags: its own bit
/// when the event has left/right Option bits, else the device-independent one.
fn right_option_down(flags: u64) -> bool {
    if flags & (flag::DEVICE_LEFT_ALTERNATE | flag::DEVICE_RIGHT_ALTERNATE) != 0 {
        flags & flag::DEVICE_RIGHT_ALTERNATE != 0
    } else {
        flags & flag::ALTERNATE != 0
    }
}

fn as_command(e: Event) -> Change {
    Change {
        keycode: if e.kind == Kind::FlagsChanged { command_key(e.keycode) } else { e.keycode },
        flags: command_flags(e.flags),
    }
}

fn as_is(e: Event) -> Change {
    Change { keycode: e.keycode, flags: e.flags }
}

/// The left Control, as the Mac has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Left {
    #[default]
    Up,
    /// Down, held back since `at`.
    Waiting { held: Event, at: Instant },
    /// Down, and the Mac got it as Command.
    AsCommand,
    /// Down as AltGr's: the Mac got it as Control.
    AsControl,
}

/// Rewrites Sunshine's events one by one, in the order they come. One per tap: a new tap starts
/// with nothing held, and the tap starts over (`StreamKeys::default()`) whenever the system
/// disabled it, since events went by unseen meanwhile.
#[derive(Debug, Default)]
pub struct StreamKeys {
    left: Left,
}

impl StreamKeys {
    /// What to do with one of Sunshine's events, arriving at `now`.
    pub fn event(&mut self, e: Event, now: Instant) -> Step {
        let left_control = e.kind == Kind::FlagsChanged && e.keycode == mac_key::CONTROL;
        let mut release_held = None;
        if let Left::Waiting { held, at } = self.left {
            // Its own repeat: still waiting. Only when the left/right bits say so: without
            // them a left Control going up while the right one is held looks the same, and an
            // up must never be dropped.
            if left_control && has_control_sides(e.flags) && left_control_down(e.flags) {
                return Step { release_held: None, this: This::Drop };
            }
            let altgr = e.kind == Kind::FlagsChanged
                && e.keycode == mac_key::RIGHT_OPTION
                && right_option_down(e.flags)
                && now.saturating_duration_since(at) <= ALTGR_WINDOW;
            if altgr {
                self.left = Left::AsControl;
                release_held = Some(as_is(held));
            } else {
                self.left = Left::AsCommand;
                release_held = Some(as_command(held));
            }
        }
        let this = if left_control {
            let down = left_control_down(e.flags);
            match (self.left, down) {
                (Left::Up, true) => {
                    self.left = Left::Waiting { held: e, at: now };
                    This::Hold
                }
                (Left::AsControl, _) => {
                    if !down {
                        self.left = Left::Up;
                    }
                    This::Pass(as_is(e))
                }
                (_, down) => {
                    if !down {
                        self.left = Left::Up;
                    }
                    This::Pass(as_command(e))
                }
            }
        } else if self.left == Left::AsControl {
            // AltGr's Control and Option, and the keys typed with them, go as they are.
            This::Pass(as_is(e))
        } else {
            This::Pass(as_command(e))
        };
        Step { release_held, this }
    }

    /// The left Control could not be held back after all (the tap could not keep a copy): it
    /// goes now, as Command, carrying the returned values, so the Mac's keys stay in step.
    pub fn cannot_hold(&mut self) -> Option<Change> {
        let Left::Waiting { held, .. } = self.left else { return None };
        self.left = Left::AsCommand;
        Some(as_command(held))
    }
}

/// The Sunshine to tap, if any: only while the PC's setting is on and a display session runs
/// (Sunshine's pid is known), and never a pid that would match the user's own keyboard (0) or
/// this app's own mouse-sharing input.
pub fn tap_for(ctrl_as_cmd: bool, sunshine: Option<u32>, own_pid: u32) -> Option<u32> {
    sunshine.filter(|&pid| ctrl_as_cmd && pid != 0 && pid != own_pid)
}

/// What the Mac's sharing status says about the PC's Ctrl as ⌘ (Mac side): what the PC last
/// said on this connection; off when nobody is connected (sharing off, or ended); and nothing
/// (`None`: keep the last value) while the link reconnects or a new connection has not said
/// yet, so a sharing reconnect in the middle of a display session keeps Ctrl as ⌘.
pub fn setting_in(status: &crate::mouse::MouseStatus) -> Option<bool> {
    use crate::mouse::MouseStatus::*;
    match status {
        Connected { uses: Some(u), .. } => Some(u.ctrl_as_cmd),
        Off | Ended(_) => Some(false),
        Connected { uses: None, .. } | Connecting | Lost(_) => None,
    }
}

/// What to do with the tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Remove any tap, then tap this Sunshine.
    Start(u32),
    Stop,
}

/// Follows the setting (from the sharing link) and Sunshine's pid (from the display session),
/// which arrive apart, and says when the tap has to change.
#[derive(Debug, Default)]
pub struct Follow {
    setting: bool,
    sunshine: Option<u32>,
    tapping: Option<u32>,
}

impl Follow {
    pub const fn new() -> Follow {
        Follow { setting: false, sunshine: None, tapping: None }
    }

    pub fn setting(&mut self, on: bool, own_pid: u32) -> Option<Action> {
        self.setting = on;
        self.settle(own_pid)
    }

    pub fn sunshine(&mut self, pid: Option<u32>, own_pid: u32) -> Option<Action> {
        self.sunshine = pid;
        self.settle(own_pid)
    }

    /// The tap asked for could not be made (no Accessibility yet, say): the next change of the
    /// setting or of Sunshine, or the same setting said again, tries again.
    pub fn start_failed(&mut self) {
        self.tapping = None;
    }

    fn settle(&mut self, own_pid: u32) -> Option<Action> {
        let want = tap_for(self.setting, self.sunshine, own_pid);
        if want == self.tapping {
            return None;
        }
        self.tapping = want;
        Some(want.map_or(Action::Stop, Action::Start))
    }
}

/// The Mac: the PC's setting changed (or the PC came or went).
#[cfg(target_os = "macos")]
pub fn follow_setting(on: bool) {
    apply(|f| f.setting(on, std::process::id()));
}

/// The Mac: Sunshine started for a display session (its pid), or stopped (`None`).
#[cfg(target_os = "macos")]
pub fn follow_sunshine(pid: Option<u32>) {
    apply(|f| f.sunshine(pid, std::process::id()));
}

#[cfg(target_os = "macos")]
fn apply(change: impl FnOnce(&mut Follow) -> Option<Action>) {
    static NOW: std::sync::Mutex<(Follow, Option<tap::Tap>)> = std::sync::Mutex::new((Follow::new(), None));
    let mut now = NOW.lock().unwrap_or_else(|e| e.into_inner());
    let Some(action) = change(&mut now.0) else { return };
    // The old tap goes first (it is inert from this moment), so two never rewrite together.
    now.1 = None;
    if let Action::Start(pid) = action {
        now.1 = tap::Tap::start(pid);
        if now.1.is_none() {
            now.0.start_failed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::flag::*;
    use super::mac_key::{COMMAND as CMD_KEY, CONTROL as CTRL_KEY, RIGHT_COMMAND, RIGHT_CONTROL, RIGHT_OPTION};
    use super::*;

    const C: u16 = 0x08;
    const SHIFT: u64 = 0x0002_0000;
    const DEVICE_LEFT_SHIFT: u64 = 0x2;
    const NUMERIC_PAD: u64 = 0x0020_0000;
    const NON_COALESCED: u64 = 0x100;

    /// Left Control down or up, as Sunshine posts it.
    fn left_ctrl(down: bool) -> Event {
        let flags = if down { CONTROL | DEVICE_LEFT_CONTROL } else { 0 };
        Event { kind: Kind::FlagsChanged, keycode: CTRL_KEY, flags }
    }

    fn key(kind: Kind, keycode: u16, flags: u64) -> Event {
        Event { kind, keycode, flags }
    }

    fn pass(keycode: u16, flags: u64) -> Step {
        Step { release_held: None, this: This::Pass(Change { keycode, flags }) }
    }

    const CMD_DOWN: Change = Change { keycode: CMD_KEY, flags: COMMAND | DEVICE_LEFT_COMMAND };

    #[test]
    fn ctrl_c_reaches_the_mac_as_command_c() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        assert_eq!(k.event(left_ctrl(true), t), Step { release_held: None, this: This::Hold });
        let ctrl = CONTROL | DEVICE_LEFT_CONTROL;
        assert_eq!(
            k.event(key(Kind::KeyDown, C, ctrl), t),
            Step { release_held: Some(CMD_DOWN), this: This::Pass(Change { keycode: C, flags: COMMAND | DEVICE_LEFT_COMMAND }) }
        );
        assert_eq!(k.event(key(Kind::KeyUp, C, ctrl), t), pass(C, COMMAND | DEVICE_LEFT_COMMAND));
        assert_eq!(k.event(left_ctrl(false), t), pass(CMD_KEY, 0));
    }

    #[test]
    fn the_right_ctrl_goes_as_the_right_command_at_once() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        let down = key(Kind::FlagsChanged, RIGHT_CONTROL, CONTROL | DEVICE_RIGHT_CONTROL);
        assert_eq!(k.event(down, t), pass(RIGHT_COMMAND, COMMAND | DEVICE_RIGHT_COMMAND));
        assert_eq!(k.event(key(Kind::FlagsChanged, RIGHT_CONTROL, 0), t), pass(RIGHT_COMMAND, 0));
    }

    #[test]
    fn a_ctrl_held_and_repeating_goes_once_as_command() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        assert_eq!(k.event(left_ctrl(true), t).this, This::Drop);
        assert_eq!(k.event(left_ctrl(true), t).this, This::Drop);
        let step = k.event(key(Kind::KeyDown, C, CONTROL | DEVICE_LEFT_CONTROL), t);
        assert_eq!(step.release_held, Some(CMD_DOWN));
        // Once it went as Command, its repeats go as Command too.
        assert_eq!(k.event(left_ctrl(true), t), pass(CMD_KEY, COMMAND | DEVICE_LEFT_COMMAND));
    }

    #[test]
    fn a_tap_of_ctrl_is_a_tap_of_command() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        assert_eq!(
            k.event(left_ctrl(false), t),
            Step { release_held: Some(CMD_DOWN), this: This::Pass(Change { keycode: CMD_KEY, flags: 0 }) }
        );
        // And the next one is held back again.
        assert_eq!(k.event(left_ctrl(true), t).this, This::Hold);
    }

    #[test]
    fn ctrl_click_and_ctrl_scroll_are_command_click_and_scroll() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        let click = key(Kind::Pointer, 0, CONTROL | DEVICE_LEFT_CONTROL | NON_COALESCED);
        assert_eq!(
            k.event(click, t),
            Step {
                release_held: Some(CMD_DOWN),
                this: This::Pass(Change { keycode: 0, flags: COMMAND | DEVICE_LEFT_COMMAND | NON_COALESCED }),
            }
        );
        assert_eq!(k.event(click, t).this, This::Pass(Change { keycode: 0, flags: COMMAND | DEVICE_LEFT_COMMAND | NON_COALESCED }));
    }

    #[test]
    fn altgr_reaches_the_mac_as_control_and_option_not_command() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        let ctrl = CONTROL | DEVICE_LEFT_CONTROL;
        let altgr = ctrl | ALTERNATE | DEVICE_RIGHT_ALTERNATE;
        assert_eq!(k.event(left_ctrl(true), t).this, This::Hold);
        let right_alt = key(Kind::FlagsChanged, RIGHT_OPTION, altgr);
        assert_eq!(
            k.event(right_alt, t + Duration::from_millis(3)),
            Step { release_held: Some(Change { keycode: CTRL_KEY, flags: ctrl }), this: This::Pass(Change { keycode: RIGHT_OPTION, flags: altgr }) }
        );
        let e = 0x0E;
        assert_eq!(k.event(key(Kind::KeyDown, e, altgr), t), pass(e, altgr));
        assert_eq!(k.event(key(Kind::KeyUp, e, altgr), t), pass(e, altgr));
        // Its own repeat stays Control.
        assert_eq!(k.event(key(Kind::FlagsChanged, CTRL_KEY, altgr), t), pass(CTRL_KEY, altgr));
        let rest = ALTERNATE | DEVICE_RIGHT_ALTERNATE;
        assert_eq!(k.event(key(Kind::FlagsChanged, CTRL_KEY, rest), t), pass(CTRL_KEY, rest));
        assert_eq!(k.event(key(Kind::FlagsChanged, RIGHT_OPTION, 0), t), pass(RIGHT_OPTION, 0));
        // The next Ctrl is Command again.
        assert_eq!(k.event(left_ctrl(true), t).this, This::Hold);
        assert_eq!(k.event(key(Kind::KeyDown, C, ctrl), t).release_held, Some(CMD_DOWN));
    }

    #[test]
    fn ctrl_then_right_option_on_purpose_is_command_and_option() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        let both = CONTROL | DEVICE_LEFT_CONTROL | ALTERNATE | DEVICE_RIGHT_ALTERNATE;
        assert_eq!(
            k.event(key(Kind::FlagsChanged, RIGHT_OPTION, both), t + Duration::from_millis(200)),
            Step {
                release_held: Some(CMD_DOWN),
                this: This::Pass(Change { keycode: RIGHT_OPTION, flags: COMMAND | DEVICE_LEFT_COMMAND | ALTERNATE | DEVICE_RIGHT_ALTERNATE }),
            }
        );
    }

    #[test]
    fn ctrl_then_left_option_is_command_and_option() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        let left_option = 0x3A;
        let step = k.event(key(Kind::FlagsChanged, left_option, CONTROL | DEVICE_LEFT_CONTROL | ALTERNATE | 0x20), t);
        assert_eq!(step.release_held, Some(CMD_DOWN));
        assert_eq!(step.this, This::Pass(Change { keycode: left_option, flags: COMMAND | DEVICE_LEFT_COMMAND | ALTERNATE | 0x20 }));
    }

    /// One way: Sunshine's Command (the Menu key; Moonlight does not send Win) stays Command.
    #[test]
    fn sunshines_command_stays_command() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        let down = key(Kind::FlagsChanged, RIGHT_COMMAND, COMMAND | DEVICE_RIGHT_COMMAND);
        assert_eq!(k.event(down, t), pass(RIGHT_COMMAND, COMMAND | DEVICE_RIGHT_COMMAND));
        assert_eq!(k.event(key(Kind::KeyDown, C, COMMAND | DEVICE_RIGHT_COMMAND), t), pass(C, COMMAND | DEVICE_RIGHT_COMMAND));
        assert_eq!(k.event(key(Kind::FlagsChanged, CMD_KEY, COMMAND | DEVICE_LEFT_COMMAND), t), pass(CMD_KEY, COMMAND | DEVICE_LEFT_COMMAND));
        assert_eq!(k.event(key(Kind::FlagsChanged, RIGHT_COMMAND, 0), t), pass(RIGHT_COMMAND, 0));
    }

    /// Ctrl and the Menu key together: both are Command.
    #[test]
    fn ctrl_with_command_held_is_just_command() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        let both = CONTROL | DEVICE_RIGHT_CONTROL | COMMAND | DEVICE_RIGHT_COMMAND;
        assert_eq!(k.event(key(Kind::KeyDown, C, both), t), pass(C, COMMAND | DEVICE_RIGHT_COMMAND));
    }

    #[test]
    fn keys_without_control_or_command_go_as_they_are() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        for flags in [0, SHIFT | DEVICE_LEFT_SHIFT, ALTERNATE | DEVICE_RIGHT_ALTERNATE, NUMERIC_PAD, NON_COALESCED] {
            assert_eq!(k.event(key(Kind::KeyDown, C, flags), t), pass(C, flags));
        }
        assert_eq!(k.event(key(Kind::FlagsChanged, 0x38, SHIFT | DEVICE_LEFT_SHIFT), t), pass(0x38, SHIFT | DEVICE_LEFT_SHIFT));
    }

    #[test]
    fn a_sunshine_without_left_right_bits_still_works() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        assert_eq!(k.event(key(Kind::FlagsChanged, CTRL_KEY, CONTROL), t).this, This::Hold);
        let step = k.event(key(Kind::KeyDown, C, CONTROL), t);
        assert_eq!(step.release_held, Some(Change { keycode: CMD_KEY, flags: COMMAND }));
        assert_eq!(step.this, This::Pass(Change { keycode: C, flags: COMMAND }));
        assert_eq!(k.event(key(Kind::FlagsChanged, CTRL_KEY, 0), t), pass(CMD_KEY, 0));
    }

    #[test]
    fn the_left_ctrl_going_up_while_the_right_one_is_held_is_seen_as_up() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        k.event(key(Kind::FlagsChanged, RIGHT_CONTROL, CONTROL | DEVICE_LEFT_CONTROL | DEVICE_RIGHT_CONTROL), t);
        // Left up: the right one's bits stay.
        let up = key(Kind::FlagsChanged, CTRL_KEY, CONTROL | DEVICE_RIGHT_CONTROL);
        assert_eq!(k.event(up, t), pass(CMD_KEY, COMMAND | DEVICE_RIGHT_COMMAND));
        assert_eq!(k.event(left_ctrl(true), t).this, This::Hold, "the left Ctrl is up again");
    }

    #[test]
    fn only_control_becomes_command_and_every_other_bit_stays() {
        let others = SHIFT | DEVICE_LEFT_SHIFT | ALTERNATE | DEVICE_RIGHT_ALTERNATE | NUMERIC_PAD | NON_COALESCED | 0x0080_0000;
        assert_eq!(command_flags(others), others);
        assert_eq!(command_flags(others | CONTROL | DEVICE_LEFT_CONTROL), others | COMMAND | DEVICE_LEFT_COMMAND);
        assert_eq!(command_flags(CONTROL | DEVICE_RIGHT_CONTROL), COMMAND | DEVICE_RIGHT_COMMAND);
        assert_eq!(command_flags(COMMAND | DEVICE_LEFT_COMMAND), COMMAND | DEVICE_LEFT_COMMAND);
        assert_eq!(command_key(CTRL_KEY), CMD_KEY);
        assert_eq!(command_key(RIGHT_CONTROL), RIGHT_COMMAND);
        for code in [CMD_KEY, RIGHT_COMMAND, RIGHT_OPTION, C] {
            assert_eq!(command_key(code), code);
        }
    }

    #[test]
    fn altgr_counts_within_50_ms_and_not_after() {
        let altgr = CONTROL | DEVICE_LEFT_CONTROL | ALTERNATE | DEVICE_RIGHT_ALTERNATE;
        let right_alt = key(Kind::FlagsChanged, RIGHT_OPTION, altgr);
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        assert_eq!(k.event(right_alt, t + Duration::from_millis(50)).this, This::Pass(Change { keycode: RIGHT_OPTION, flags: altgr }));
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        let step = k.event(right_alt, t + Duration::from_millis(51));
        assert_eq!(step.release_held, Some(CMD_DOWN));
    }

    /// The right Option going up (the left one still held) is no AltGr.
    #[test]
    fn a_right_option_going_up_after_a_ctrl_is_no_altgr() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        let left_still_held = CONTROL | DEVICE_LEFT_CONTROL | ALTERNATE | DEVICE_LEFT_ALTERNATE;
        let step = k.event(key(Kind::FlagsChanged, RIGHT_OPTION, left_still_held), t);
        assert_eq!(step.release_held, Some(CMD_DOWN));
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        let step = k.event(key(Kind::FlagsChanged, RIGHT_OPTION, CONTROL | DEVICE_LEFT_CONTROL), t);
        assert_eq!(step.release_held, Some(CMD_DOWN));
    }

    #[test]
    fn clicks_and_keys_while_altgr_is_held_go_as_they_are() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        let altgr = CONTROL | DEVICE_LEFT_CONTROL | ALTERNATE | DEVICE_RIGHT_ALTERNATE;
        k.event(left_ctrl(true), t);
        k.event(key(Kind::FlagsChanged, RIGHT_OPTION, altgr), t);
        assert_eq!(k.event(key(Kind::Pointer, 0, altgr), t), pass(0, altgr));
        assert_eq!(k.event(key(Kind::KeyDown, C, altgr | SHIFT), t), pass(C, altgr | SHIFT));
        assert_eq!(k.event(key(Kind::FlagsChanged, 0x38, altgr | SHIFT), t), pass(0x38, altgr | SHIFT));
    }

    /// Without left/right bits, a left Control going up while the right one is held still
    /// says Control: it is never dropped as a repeat, and the Mac gets ⌘ down then it.
    #[test]
    fn without_left_right_bits_a_left_ctrl_up_is_never_dropped() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        assert_eq!(k.event(key(Kind::FlagsChanged, CTRL_KEY, CONTROL), t).this, This::Hold);
        let step = k.event(key(Kind::FlagsChanged, CTRL_KEY, CONTROL), t);
        assert_eq!(step.release_held, Some(Change { keycode: CMD_KEY, flags: COMMAND }));
        assert_eq!(step.this, This::Pass(Change { keycode: CMD_KEY, flags: COMMAND }));
    }

    /// `Drop` is only ever a repeat of a Control held back: never an event that lets a key go.
    #[test]
    fn nothing_that_lets_a_key_go_is_ever_dropped() {
        let ups = [
            key(Kind::FlagsChanged, CTRL_KEY, 0),
            key(Kind::FlagsChanged, CTRL_KEY, CONTROL | DEVICE_RIGHT_CONTROL),
            key(Kind::FlagsChanged, CTRL_KEY, CONTROL),
            key(Kind::FlagsChanged, RIGHT_CONTROL, CONTROL | DEVICE_LEFT_CONTROL),
            key(Kind::KeyUp, C, CONTROL | DEVICE_LEFT_CONTROL),
            key(Kind::Pointer, 0, CONTROL | DEVICE_LEFT_CONTROL),
        ];
        let starts: [&[Event]; 3] = [
            &[],
            &[left_ctrl(true)],
            &[left_ctrl(true), key(Kind::FlagsChanged, RIGHT_OPTION, CONTROL | DEVICE_LEFT_CONTROL | ALTERNATE | DEVICE_RIGHT_ALTERNATE)],
        ];
        for start in starts {
            for up in ups {
                let (mut k, t) = (StreamKeys::default(), Instant::now());
                for e in start {
                    k.event(*e, t);
                }
                assert_ne!(k.event(up, t).this, This::Drop, "{up:?} after {start:?}");
            }
        }
    }

    #[test]
    fn a_ctrl_that_cannot_be_held_goes_at_once_as_command() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        assert_eq!(k.cannot_hold(), None, "nothing held");
        k.event(left_ctrl(true), t);
        assert_eq!(k.cannot_hold(), Some(CMD_DOWN));
        // Its up goes as ⌘ up, and the next Ctrl is held back again.
        assert_eq!(k.event(left_ctrl(false), t), pass(CMD_KEY, 0));
        assert_eq!(k.event(left_ctrl(true), t).this, This::Hold);
    }

    /// The tap starts over after the system disabled it: whatever was held is forgotten, and a
    /// Control going up that it never saw go down still goes as ⌘ up.
    #[test]
    fn starting_over_leaves_nothing_held() {
        let (mut k, t) = (StreamKeys::default(), Instant::now());
        k.event(left_ctrl(true), t);
        k = StreamKeys::default();
        assert_eq!(k.event(left_ctrl(false), t), pass(CMD_KEY, 0));
        assert_eq!(k.event(key(Kind::KeyDown, C, 0), t), pass(C, 0));
    }

    #[test]
    fn the_tap_runs_only_with_the_setting_on_and_a_sunshine_to_match() {
        let own = 500;
        assert_eq!(tap_for(true, Some(812), own), Some(812));
        assert_eq!(tap_for(false, Some(812), own), None, "setting off");
        assert_eq!(tap_for(true, None, own), None, "no display session");
        assert_eq!(tap_for(true, Some(0), own), None, "pid 0 could match the user's own keyboard");
        assert_eq!(tap_for(true, Some(own), own), None, "our own mouse-sharing input");
    }

    #[test]
    fn follow_starts_and_stops_the_tap_as_the_two_arrive() {
        let own = 500;
        let mut f = Follow::new();
        assert_eq!(f.setting(true, own), None, "no display yet");
        assert_eq!(f.sunshine(Some(812), own), Some(Action::Start(812)));
        assert_eq!(f.setting(true, own), None, "the same setting again changes nothing");
        assert_eq!(f.setting(false, own), Some(Action::Stop));
        assert_eq!(f.setting(true, own), Some(Action::Start(812)));
        assert_eq!(f.sunshine(Some(900), own), Some(Action::Start(900)), "a new Sunshine: a new tap");
        assert_eq!(f.sunshine(None, own), Some(Action::Stop));
        assert_eq!(f.sunshine(None, own), None);
    }

    #[test]
    fn a_tap_that_could_not_be_made_is_tried_again_on_the_next_word() {
        let own = 500;
        let mut f = Follow::new();
        f.setting(true, own);
        assert_eq!(f.sunshine(Some(812), own), Some(Action::Start(812)));
        f.start_failed();
        assert_eq!(f.setting(true, own), Some(Action::Start(812)), "the same setting said again");
        f.start_failed();
        assert_eq!(f.sunshine(Some(812), own), Some(Action::Start(812)), "the session said again");
        assert_eq!(f.setting(true, own), None, "made this time: nothing more");
        f.start_failed();
        assert_eq!(f.setting(false, own), None, "off, and nothing is running");
    }

    #[test]
    fn the_mac_takes_the_setting_from_what_the_connected_pc_said() {
        use crate::mouse::{EndReason, MouseStatus, Uses};
        let connected = |uses: Option<Uses>| MouseStatus::Connected {
            mac_id: "pc".into(),
            mac_name: "Desk".into(),
            uses,
            problem: None,
            from: None,
            rtt_ms: None,
        };
        let on = Uses { mouse: false, clipboard: false, display: true, ctrl_as_cmd: true };
        assert_eq!(setting_in(&connected(Some(on))), Some(true));
        assert_eq!(setting_in(&connected(Some(Uses { ctrl_as_cmd: false, ..on }))), Some(false));
        // A reconnect keeps the last value: lost, waiting, or connected but not said yet.
        assert_eq!(setting_in(&connected(None)), None);
        assert_eq!(setting_in(&MouseStatus::Lost("gone".into())), None);
        assert_eq!(setting_in(&MouseStatus::Connecting), None);
        // Nobody connected: off.
        assert_eq!(setting_in(&MouseStatus::Off), Some(false));
        assert_eq!(setting_in(&MouseStatus::Ended(EndReason::ByMac)), Some(false));
    }
}

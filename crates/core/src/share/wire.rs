//! What the PC and the Mac say to each other over the input channel.

use super::geometry::HandBack;
use crate::mouse::Uses;
use crate::side::Side;
use input_event::{Event, KeyboardEvent, PointerEvent};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One mouse or keyboard event, as captured on the PC.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Input {
    Motion { dx: f64, dy: f64 },
    Button { button: u32, state: u32 },
    Axis { axis: u8, value: f64 },
    AxisDiscrete { axis: u8, value: i32 },
    Key { key: u32, state: u8 },
    Modifiers { depressed: u32, latched: u32, locked: u32, group: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToMac {
    /// The PC's pointer crossed over. `pc_side`: where the PC is, seen from the Mac.
    /// `along`: how far along that edge (0..1) it crossed.
    Enter { pc_side: Side, along: f64 },
    Input(Input),
    /// The PC took control back itself (release shortcut, or shutting down).
    Leave,
    /// The PC's keyboard language ("he", "en"): the Mac types in it.
    Language(String),
    /// Clipboard contents copied on the PC.
    Clip(crate::clipboard::ClipMsg),
    /// Heartbeat: the Mac answers with `Pong`. Either side gives up after `SILENCE` without
    /// hearing from the other.
    Ping,
    /// The PC ended the connection.
    Bye,
    /// What the PC is using the Mac for (shown on the Mac); sent on connect and when it changes.
    /// `ctrl_as_cmd`: the PC's Ctrl as ⌘ setting, which the Mac applies to typing into the
    /// display window (`keys_stream`). An extra field: an older Mac skips it, and an older PC
    /// does not send it (off).
    Uses {
        mouse: bool,
        clipboard: bool,
        display: bool,
        #[serde(default)]
        ctrl_as_cmd: bool,
    },
    /// The PC's smoothed round trip to the Mac, in ms (shown on the Mac): the first measurement,
    /// then only when `latency_due` says so. Never measured while nothing is in use.
    Latency { ms: u32 },
}

/// The round trip is reported (to the PC's own status and to the Mac) at most this often...
pub const LATENCY_EVERY: Duration = Duration::from_secs(30);
/// ...and only when it moved by this many ms since it was last reported.
pub const LATENCY_STEP: u32 = 3;

/// Whether a round trip of `ms` is worth reporting, given the last report (its value and how
/// long ago it was), if any: the first measurement always is, later ones only at most every
/// `LATENCY_EVERY` and when they moved by `LATENCY_STEP` or more, so the number stays steady.
pub fn latency_due(last: Option<(u32, Duration)>, ms: u32) -> bool {
    match last {
        None => true,
        Some((was, ago)) => ago >= LATENCY_EVERY && was.abs_diff(ms) >= LATENCY_STEP,
    }
}

/// While something is in use: how often the PC checks the Mac is still there...
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// ...and how long without a word before a side treats the other as gone.
pub const SILENCE: Duration = Duration::from_secs(3);
/// The same while nothing is in use (mouse and keyboard off, no display): the link stays up
/// with next to no traffic.
pub const IDLE_HEARTBEAT: Duration = Duration::from_secs(15);
pub const IDLE_SILENCE: Duration = Duration::from_secs(45);

/// The heartbeat and silence a link keeps, following what the PC uses the Mac for. Both sides
/// derive it from the same `Uses` (the PC from what it sends, the Mac from what it last
/// heard), so neither gives up on the other early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    pub heartbeat: Duration,
    pub silence: Duration,
}

impl Pace {
    pub const BUSY: Pace = Pace { heartbeat: HEARTBEAT, silence: SILENCE };
    pub const IDLE: Pace = Pace { heartbeat: IDLE_HEARTBEAT, silence: IDLE_SILENCE };

    /// Fast while anything is in use; slow when nothing is, or nothing has been said.
    pub fn of(uses: Option<Uses>) -> Pace {
        match uses {
            Some(u) if u.mouse || u.clipboard || u.display => Pace::BUSY,
            _ => Pace::IDLE,
        }
    }

    pub fn is_idle(self) -> bool {
        self == Pace::IDLE
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToPc {
    HandBack(HandBack),
    Pong,
    /// Clipboard contents copied on the Mac.
    Clip(crate::clipboard::ClipMsg),
    /// The Mac ended the connection; the PC stays disconnected until told otherwise.
    Disconnect,
}

impl From<Event> for Input {
    fn from(e: Event) -> Input {
        match e {
            Event::Pointer(PointerEvent::Motion { dx, dy, .. }) => Input::Motion { dx, dy },
            Event::Pointer(PointerEvent::Button { button, state, .. }) => Input::Button { button, state },
            Event::Pointer(PointerEvent::Axis { axis, value, .. }) => Input::Axis { axis, value },
            Event::Pointer(PointerEvent::AxisDiscrete120 { axis, value }) => Input::AxisDiscrete { axis, value },
            Event::Keyboard(KeyboardEvent::Key { key, state, .. }) => Input::Key { key, state },
            Event::Keyboard(KeyboardEvent::Modifiers { depressed, latched, locked, group }) => {
                Input::Modifiers { depressed, latched, locked, group }
            }
        }
    }
}

impl From<Input> for Event {
    fn from(i: Input) -> Event {
        match i {
            Input::Motion { dx, dy } => Event::Pointer(PointerEvent::Motion { time: 0, dx, dy }),
            Input::Button { button, state } => Event::Pointer(PointerEvent::Button { time: 0, button, state }),
            Input::Axis { axis, value } => Event::Pointer(PointerEvent::Axis { time: 0, axis, value }),
            Input::AxisDiscrete { axis, value } => Event::Pointer(PointerEvent::AxisDiscrete120 { axis, value }),
            Input::Key { key, state } => Event::Keyboard(KeyboardEvent::Key { time: 0, key, state }),
            Input::Modifiers { depressed, latched, locked, group } => {
                Event::Keyboard(KeyboardEvent::Modifiers { depressed, latched, locked, group })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_input_survives_the_trip() {
        let all = [
            Input::Motion { dx: 1.5, dy: -2.0 },
            Input::Button { button: 0x110, state: 1 },
            Input::Axis { axis: 0, value: 3.0 },
            Input::AxisDiscrete { axis: 1, value: -120 },
            Input::Key { key: 30, state: 1 },
            Input::Modifiers { depressed: 1, latched: 0, locked: 2, group: 0 },
        ];
        for input in all {
            let json = serde_json::to_string(&ToMac::Input(input)).unwrap();
            let back: ToMac = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ToMac::Input(input));
            assert_eq!(Input::from(Event::from(input)), input);
        }
    }

    #[test]
    fn the_control_messages_survive_the_trip() {
        for m in [ToMac::Bye, ToMac::Uses { mouse: true, clipboard: false, display: true, ctrl_as_cmd: true }, ToMac::Latency { ms: 42 }] {
            let back: ToMac = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
            assert_eq!(back, m);
        }
        let back: ToPc = serde_json::from_str(&serde_json::to_string(&ToPc::Disconnect).unwrap()).unwrap();
        assert_eq!(back, ToPc::Disconnect);
    }

    /// An older PC says nothing about Ctrl as ⌘: the Mac takes it as off (no tap).
    #[test]
    fn uses_from_an_older_pc_reads_as_ctrl_as_cmd_off() {
        let old = r#"{"Uses":{"mouse":true,"clipboard":false,"display":true}}"#;
        let m: ToMac = serde_json::from_str(old).unwrap();
        assert_eq!(m, ToMac::Uses { mouse: true, clipboard: false, display: true, ctrl_as_cmd: false });
    }

    /// An older Mac reads the new field as an extra it skips: the connection goes on as before.
    #[test]
    fn an_older_mac_skips_the_new_field() {
        #[derive(Debug, PartialEq, Deserialize)]
        enum OldToMac {
            Uses { mouse: bool, clipboard: bool, display: bool },
            Ping,
        }
        let new = serde_json::to_string(&ToMac::Uses { mouse: false, clipboard: true, display: true, ctrl_as_cmd: true }).unwrap();
        assert_eq!(new, r#"{"Uses":{"mouse":false,"clipboard":true,"display":true,"ctrl_as_cmd":true}}"#);
        let old: OldToMac = serde_json::from_str(&new).unwrap();
        assert_eq!(old, OldToMac::Uses { mouse: false, clipboard: true, display: true });
        assert_eq!(serde_json::from_str::<OldToMac>(r#""Ping""#).unwrap(), OldToMac::Ping);
    }

    fn uses(mouse: bool, clipboard: bool, display: bool) -> Option<Uses> {
        Some(Uses { mouse, clipboard, display, ctrl_as_cmd: false })
    }

    #[test]
    fn nothing_in_use_is_the_slow_pair_and_anything_in_use_the_fast_one() {
        assert_eq!(Pace::of(uses(false, false, false)), Pace::IDLE);
        assert_eq!(Pace::of(uses(true, false, false)), Pace::BUSY);
        assert_eq!(Pace::of(uses(true, true, false)), Pace::BUSY);
        assert_eq!(Pace::of(uses(false, false, true)), Pace::BUSY);
        assert_eq!(Pace::of(uses(true, true, true)), Pace::BUSY);
        // Before the PC has said (and with nobody connected) nothing is in use.
        assert_eq!(Pace::of(None), Pace::IDLE);
        assert_eq!(Pace::IDLE, Pace { heartbeat: Duration::from_secs(15), silence: Duration::from_secs(45) });
        assert_eq!(Pace::BUSY, Pace { heartbeat: Duration::from_secs(1), silence: Duration::from_secs(3) });
        assert!(Pace::IDLE.is_idle() && !Pace::BUSY.is_idle());
    }

    #[test]
    fn the_first_round_trip_is_reported_at_once() {
        assert!(latency_due(None, 12));
    }

    #[test]
    fn a_round_trip_is_reported_at_most_every_30_s_and_only_when_it_moved_3_ms() {
        let s = Duration::from_secs;
        assert!(!latency_due(Some((10, s(5))), 40), "too soon, however much it moved");
        assert!(!latency_due(Some((10, s(29))), 20), "still too soon");
        assert!(latency_due(Some((10, s(30))), 13), "30 s on and 3 ms off");
        assert!(latency_due(Some((10, s(30))), 7), "or 3 ms the other way");
        assert!(!latency_due(Some((10, s(30))), 12), "2 ms is no change");
        assert!(!latency_due(Some((10, s(600))), 8), "however long it has been");
        assert!(latency_due(Some((10, s(95))), 30));
    }
}

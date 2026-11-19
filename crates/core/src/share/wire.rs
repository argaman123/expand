//! What the PC and the Mac say to each other over the input channel.

use super::geometry::HandBack;
use crate::side::Side;
use input_event::{Event, KeyboardEvent, PointerEvent};
use serde::{Deserialize, Serialize};

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
}

/// How often the PC checks the Mac is still there.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(1);
/// How long without a word before a side treats the other as gone.
pub const SILENCE: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToPc {
    HandBack(HandBack),
    Pong,
    /// Clipboard contents copied on the Mac.
    Clip(crate::clipboard::ClipMsg),
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
}

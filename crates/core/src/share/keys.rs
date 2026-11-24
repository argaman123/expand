//! Ctrl as ⌘: while the Mac has the PC's keyboard, the PC's Ctrl keys reach the Mac as Command
//! and its Windows keys as Control, so Ctrl+C, Ctrl+V and the rest do there what they do on the
//! PC. Alt stays Option.
//!
//! Done on the PC, on the way out: the setting lives in the PC's config (no new message, and an
//! older Mac works as it is), and the PC's own shortcuts (the return chord, the language
//! shortcuts) are read from the keys as pressed, before this.
//!
//! AltGr is not Ctrl: Windows sends it as a left Ctrl going down just before the right Alt. So a
//! left Ctrl going down waits for the next input; with the right Alt right after it, both go as
//! they are (Control and Option, as before), or AltGr's characters (₪, niqqud) would become ⌘⌥
//! shortcuts on the Mac.

use super::wire::Input;
use std::collections::HashMap;

/// Linux key codes, as the PC's capture reports them.
mod key {
    pub const LEFT_CTRL: u32 = 29;
    pub const RIGHT_CTRL: u32 = 97;
    pub const LEFT_META: u32 = 125;
    pub const RIGHT_META: u32 = 126;
    pub const RIGHT_ALT: u32 = 100;
}

/// The modifier bits of a `Modifiers` event (X11's masks, as Lan Mouse uses them).
const CONTROL_MASK: u32 = 1 << 2;
const MOD4_MASK: u32 = 1 << 6;

/// The key the Mac gets for `code` with the swap on.
fn swapped(code: u32) -> u32 {
    match code {
        key::LEFT_CTRL => key::LEFT_META,
        key::RIGHT_CTRL => key::RIGHT_META,
        key::LEFT_META => key::LEFT_CTRL,
        key::RIGHT_META => key::RIGHT_CTRL,
        other => other,
    }
}

/// Turns the PC's input into what the Mac gets. A key is let go as the key it went down as,
/// whatever the setting is by then, so turning it over while a key is held leaves nothing stuck.
#[derive(Debug, Default)]
pub struct CmdKeys {
    on: bool,
    /// Keys down now: as pressed on the PC, and as sent to the Mac.
    held: HashMap<u32, u32>,
    /// A left Ctrl went down and is not sent yet: it may be AltGr's.
    ctrl_waiting: bool,
}

impl CmdKeys {
    pub fn new(on: bool) -> CmdKeys {
        CmdKeys { on, ..CmdKeys::default() }
    }

    /// Turns the swap on or off for keys pressed from now on.
    pub fn set(&mut self, on: bool) {
        self.on = on;
    }

    /// What the Mac gets for one input from the PC: nothing (a left Ctrl waiting to see whether
    /// it is AltGr's), that input, or a waiting Ctrl first and then it.
    pub fn input(&mut self, input: Input) -> Vec<Input> {
        let mut out = Vec::new();
        if self.ctrl_waiting {
            match input {
                // AltGr: Ctrl and Alt go as they are.
                Input::Key { key: key::RIGHT_ALT, state: 1 } => {
                    self.ctrl_waiting = false;
                    self.held.insert(key::LEFT_CTRL, key::LEFT_CTRL);
                    out.push(Input::Key { key: key::LEFT_CTRL, state: 1 });
                }
                // Its own repeat: still waiting.
                Input::Key { key: key::LEFT_CTRL, state: 1 } => return out,
                // Anything else: Ctrl was Ctrl, so ⌘ goes first.
                _ => {
                    self.ctrl_waiting = false;
                    let sent = self.press(key::LEFT_CTRL);
                    out.push(Input::Key { key: sent, state: 1 });
                }
            }
        }
        match input {
            Input::Key { key: key::LEFT_CTRL, state: 1 } if self.on && !self.held.contains_key(&key::LEFT_CTRL) => {
                self.ctrl_waiting = true;
            }
            Input::Key { key, state: 0 } => out.push(Input::Key { key: self.release(key).unwrap_or(key), state: 0 }),
            Input::Key { key, state } => out.push(Input::Key { key: self.press(key), state }),
            Input::Modifiers { depressed, latched, locked, group } => out.push(Input::Modifiers {
                depressed: self.mask(depressed),
                latched: self.mask(latched),
                locked: self.mask(locked),
                group,
            }),
            other => out.push(other),
        }
        out
    }

    /// The key sent for the PC's `code` going down; a repeat of a held key goes as it first went.
    fn press(&mut self, code: u32) -> u32 {
        let sent = if self.on { swapped(code) } else { code };
        *self.held.entry(code).or_insert(sent)
    }

    /// The key to let go for the PC's `code` (taking control back lets go of every key held):
    /// as it went down. `None` for a key the Mac never got (a left Ctrl still waiting).
    pub fn release(&mut self, code: u32) -> Option<u32> {
        if code == key::LEFT_CTRL && self.ctrl_waiting {
            self.ctrl_waiting = false;
            return None;
        }
        Some(self.held.remove(&code).unwrap_or(if self.on { swapped(code) } else { code }))
    }

    /// The modifier mask to send for the PC's `depressed` (or latched, locked) mask.
    fn mask(&self, mask: u32) -> u32 {
        if !self.on {
            return mask;
        }
        let control = mask & CONTROL_MASK != 0;
        let mod4 = mask & MOD4_MASK != 0;
        let rest = mask & !(CONTROL_MASK | MOD4_MASK);
        rest | if control { MOD4_MASK } else { 0 } | if mod4 { CONTROL_MASK } else { 0 }
    }

    /// Forgets the held keys (control came back to the PC, or crossed over anew).
    pub fn clear(&mut self) {
        self.held.clear();
        self.ctrl_waiting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::key::*;
    use super::*;

    const C: u32 = 46;
    const LEFT_ALT: u32 = 56;

    fn down(key: u32) -> Input {
        Input::Key { key, state: 1 }
    }

    fn up(key: u32) -> Input {
        Input::Key { key, state: 0 }
    }

    /// Everything the Mac gets for these inputs, in order.
    fn through(k: &mut CmdKeys, inputs: &[Input]) -> Vec<Input> {
        inputs.iter().flat_map(|i| k.input(*i)).collect()
    }

    #[test]
    fn ctrl_c_reaches_the_mac_as_command_c() {
        let mut k = CmdKeys::new(true);
        assert_eq!(
            through(&mut k, &[down(LEFT_CTRL), down(C), up(C), up(LEFT_CTRL)]),
            [down(LEFT_META), down(C), up(C), up(LEFT_META)]
        );
        assert_eq!(through(&mut k, &[down(RIGHT_CTRL), up(RIGHT_CTRL)]), [down(RIGHT_META), up(RIGHT_META)]);
    }

    #[test]
    fn a_ctrl_held_and_repeating_goes_once_as_command() {
        let mut k = CmdKeys::new(true);
        assert_eq!(
            through(&mut k, &[down(LEFT_CTRL), down(LEFT_CTRL), down(LEFT_CTRL), down(C)]),
            [down(LEFT_META), down(C)]
        );
    }

    #[test]
    fn ctrl_click_is_command_click() {
        let mut k = CmdKeys::new(true);
        let click = Input::Button { button: 0x110, state: 1 };
        assert_eq!(through(&mut k, &[down(LEFT_CTRL), click]), [down(LEFT_META), click]);
    }

    #[test]
    fn a_tap_of_ctrl_is_a_tap_of_command() {
        let mut k = CmdKeys::new(true);
        assert_eq!(through(&mut k, &[down(LEFT_CTRL), up(LEFT_CTRL)]), [down(LEFT_META), up(LEFT_META)]);
    }

    #[test]
    fn altgr_reaches_the_mac_as_control_and_option_not_command() {
        // Windows: AltGr is a left Ctrl going down just before the right Alt.
        let mut k = CmdKeys::new(true);
        let e = 18;
        assert_eq!(
            through(&mut k, &[down(LEFT_CTRL), down(RIGHT_ALT), down(e), up(e), up(LEFT_CTRL), up(RIGHT_ALT)]),
            [down(LEFT_CTRL), down(RIGHT_ALT), down(e), up(e), up(LEFT_CTRL), up(RIGHT_ALT)]
        );
        // The next Ctrl is Command again.
        assert_eq!(through(&mut k, &[down(LEFT_CTRL), down(C)]), [down(LEFT_META), down(C)]);
    }

    #[test]
    fn the_windows_key_reaches_the_mac_as_control() {
        let mut k = CmdKeys::new(true);
        assert_eq!(through(&mut k, &[down(LEFT_META), up(LEFT_META)]), [down(LEFT_CTRL), up(LEFT_CTRL)]);
        assert_eq!(through(&mut k, &[down(RIGHT_META)]), [down(RIGHT_CTRL)]);
    }

    #[test]
    fn alt_stays_option() {
        let mut k = CmdKeys::new(true);
        assert_eq!(through(&mut k, &[down(LEFT_ALT)]), [down(LEFT_ALT)]);
    }

    #[test]
    fn off_sends_the_keys_as_they_are() {
        let mut k = CmdKeys::new(false);
        assert_eq!(through(&mut k, &[down(LEFT_CTRL), down(LEFT_META)]), [down(LEFT_CTRL), down(LEFT_META)]);
    }

    #[test]
    fn a_key_held_while_the_setting_changes_is_let_go_as_it_went_down() {
        let mut k = CmdKeys::new(true);
        assert_eq!(through(&mut k, &[down(LEFT_CTRL), down(C)]), [down(LEFT_META), down(C)]);
        k.set(false);
        assert_eq!(through(&mut k, &[down(LEFT_CTRL)]), [down(LEFT_META)], "a repeat goes as it first went");
        assert_eq!(through(&mut k, &[up(LEFT_CTRL)]), [up(LEFT_META)], "Command must not stay down on the Mac");
        assert_eq!(through(&mut k, &[down(LEFT_CTRL)]), [down(LEFT_CTRL)]);
    }

    #[test]
    fn taking_control_back_lets_go_as_the_keys_went_down_and_skips_a_waiting_ctrl() {
        let mut k = CmdKeys::new(true);
        through(&mut k, &[down(LEFT_CTRL), down(C)]);
        assert_eq!(k.release(LEFT_CTRL), Some(LEFT_META));
        assert_eq!(k.release(C), Some(C));
        // A Ctrl still waiting never reached the Mac.
        through(&mut k, &[down(LEFT_CTRL)]);
        assert_eq!(k.release(LEFT_CTRL), None);
    }

    #[test]
    fn the_modifier_masks_swap_control_and_command() {
        let mut k = CmdKeys::new(true);
        let shift = 1;
        let m = |d: u32| Input::Modifiers { depressed: d, latched: 0, locked: 2, group: 0 };
        assert_eq!(k.input(m(CONTROL_MASK | shift)), [m(MOD4_MASK | shift)]);
        assert_eq!(k.input(m(MOD4_MASK)), [m(CONTROL_MASK)]);
        assert_eq!(k.input(m(CONTROL_MASK | MOD4_MASK)), [m(CONTROL_MASK | MOD4_MASK)]);
        assert_eq!(CmdKeys::new(false).input(m(CONTROL_MASK)), [m(CONTROL_MASK)]);
    }
}

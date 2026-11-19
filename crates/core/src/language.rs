//! Keyboard language: while the PC's keyboard types on the Mac, the Mac types in the PC's
//! language. The PC's language shortcuts (Alt+Shift, Win+Space), pressed while the Mac has the
//! keyboard, switch the language on both instead of reaching the Mac as stray keys.

/// "he-IL" -> "he": the part both systems agree on.
pub fn primary_language(tag: &str) -> String {
    tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase()
}

/// What to do with a key event from the PC's keyboard while the Mac has control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Send it to the Mac as usual.
    Forward,
    /// Send it, and switch the language (Alt+Shift: the Mac ignores those keys anyway).
    ForwardAndSwitch,
    /// Keep it from the Mac (Win+Space would open Spotlight), and switch the language.
    SwallowAndSwitch,
    /// Keep it from the Mac (the release of a swallowed key).
    Swallow,
}

/// Linux key codes, as the PC's capture reports them.
mod key {
    pub const LEFT_SHIFT: u32 = 42;
    pub const RIGHT_SHIFT: u32 = 54;
    pub const LEFT_ALT: u32 = 56;
    pub const RIGHT_ALT: u32 = 100;
    pub const LEFT_META: u32 = 125;
    pub const RIGHT_META: u32 = 126;
    pub const SPACE: u32 = 57;
}

/// Spots the PC's language shortcuts in the stream of key events.
#[derive(Default)]
pub struct Shortcuts {
    alt: bool,
    shift: bool,
    meta: bool,
    /// Alt and Shift went down together with nothing else: switch when one is let go.
    alt_shift_armed: bool,
    swallowed_space: bool,
}

impl Shortcuts {
    /// `state`: 1 pressed, 0 released.
    pub fn key(&mut self, code: u32, state: u8) -> KeyAction {
        let down = state != 0;
        match code {
            key::LEFT_ALT | key::RIGHT_ALT | key::LEFT_SHIFT | key::RIGHT_SHIFT => {
                let was_both = self.alt && self.shift;
                if matches!(code, key::LEFT_ALT | key::RIGHT_ALT) {
                    self.alt = down;
                } else {
                    self.shift = down;
                }
                if down {
                    // Arm only when the second of the two goes down with nothing else around.
                    if self.alt && self.shift && !self.meta {
                        self.alt_shift_armed = true;
                    }
                    KeyAction::Forward
                } else if was_both && self.alt_shift_armed {
                    self.alt_shift_armed = false;
                    KeyAction::ForwardAndSwitch
                } else {
                    KeyAction::Forward
                }
            }
            key::LEFT_META | key::RIGHT_META => {
                self.meta = down;
                self.alt_shift_armed = false;
                KeyAction::Forward
            }
            key::SPACE if down && self.meta => {
                self.swallowed_space = true;
                KeyAction::SwallowAndSwitch
            }
            key::SPACE if !down && self.swallowed_space => {
                self.swallowed_space = false;
                KeyAction::Swallow
            }
            _ => {
                // Any other key makes Alt+Shift a shortcut of its own.
                self.alt_shift_armed = false;
                KeyAction::Forward
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use key::*;

    const A: u32 = 30;

    fn run(events: &[(u32, u8)]) -> Vec<KeyAction> {
        let mut s = Shortcuts::default();
        events.iter().map(|&(k, st)| s.key(k, st)).collect()
    }

    #[test]
    fn languages_compare_by_their_first_part() {
        assert_eq!(primary_language("he-IL"), "he");
        assert_eq!(primary_language("en-US"), "en");
        assert_eq!(primary_language("EN"), "en");
        assert_eq!(primary_language("zh_Hans"), "zh");
    }

    #[test]
    fn ordinary_typing_goes_through() {
        assert!(run(&[(A, 1), (A, 0), (LEFT_SHIFT, 1), (A, 1), (A, 0), (LEFT_SHIFT, 0)])
            .iter()
            .all(|a| *a == KeyAction::Forward));
    }

    #[test]
    fn alt_shift_switches_when_let_go() {
        assert_eq!(
            run(&[(LEFT_ALT, 1), (LEFT_SHIFT, 1), (LEFT_SHIFT, 0), (LEFT_ALT, 0)]),
            vec![KeyAction::Forward, KeyAction::Forward, KeyAction::ForwardAndSwitch, KeyAction::Forward]
        );
    }

    #[test]
    fn shift_then_alt_also_switches() {
        let actions = run(&[(RIGHT_SHIFT, 1), (LEFT_ALT, 1), (LEFT_ALT, 0), (RIGHT_SHIFT, 0)]);
        assert_eq!(actions.iter().filter(|a| **a == KeyAction::ForwardAndSwitch).count(), 1);
    }

    // Alt+Shift+A is a shortcut of its own, not a language switch.
    #[test]
    fn alt_shift_with_another_key_does_not_switch() {
        let actions = run(&[(LEFT_ALT, 1), (LEFT_SHIFT, 1), (A, 1), (A, 0), (LEFT_SHIFT, 0), (LEFT_ALT, 0)]);
        assert!(actions.iter().all(|a| *a == KeyAction::Forward), "{actions:?}");
    }

    #[test]
    fn win_space_switches_and_space_never_reaches_the_mac() {
        assert_eq!(
            run(&[(LEFT_META, 1), (SPACE, 1), (SPACE, 0), (LEFT_META, 0)]),
            vec![KeyAction::Forward, KeyAction::SwallowAndSwitch, KeyAction::Swallow, KeyAction::Forward]
        );
    }

    #[test]
    fn space_alone_is_just_a_space() {
        assert_eq!(run(&[(SPACE, 1), (SPACE, 0)]), vec![KeyAction::Forward, KeyAction::Forward]);
        let _ = RIGHT_META;
        let _ = RIGHT_ALT;
    }
}

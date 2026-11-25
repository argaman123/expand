//! The Mac's side of mouse sharing: follows the PC's mouse and keyboard and says when control
//! goes back. The screens and the replaying of keys and buttons are behind traits, so all of
//! this is tested without a Mac.

use super::geometry::{hand_back, landing_on_mac, step, Area, HandBack};
use super::wire::{Input, ToMac};
use crate::side::Side;
use async_trait::async_trait;

/// The Mac's screens and pointer.
pub trait MacDesk {
    /// The MacBook's own screen.
    fn main(&self) -> Area;
    /// Every screen, the MacBook's included.
    fn displays(&self) -> Vec<Area>;
    /// Expand's virtual display, when it exists.
    fn virtual_display(&self) -> Option<Area>;
    fn pointer(&self) -> (f64, f64);
    /// Whether the left button is down for anyone (the display stream's virtual mouse too).
    fn button_held(&self) -> bool;
    /// Puts the pointer somewhere without it counting as a move.
    fn warp(&mut self, to: (f64, f64));
    /// Moves the pointer as a mouse would, dragging whatever is held.
    fn move_to(&mut self, to: (f64, f64), delta: (f64, f64), dragging: bool);
}

/// Replays keys, buttons and scrolling.
#[async_trait]
pub trait Replay: Send {
    async fn replay(&mut self, input: Input);
    /// Lets go of every key and button still held.
    async fn release_all(&mut self);
    /// Switches the keyboard language ("he", "en").
    fn set_language(&mut self, language: &str);
}

/// What a Mac that cannot replay input (no Accessibility permission) answers: the pointer
/// entering is handed straight back where it left, so the PC never captures its mouse and
/// keyboard for a Mac that would drop them.
pub fn bounce(message: &ToMac) -> Option<HandBack> {
    match message {
        ToMac::Enter { along, .. } => Some(HandBack { on_virtual: None, along: *along }),
        _ => None,
    }
}

pub struct Follower {
    active: bool,
    pc_side: Side,
}

impl Default for Follower {
    fn default() -> Self {
        Follower { active: false, pc_side: Side::Left }
    }
}

impl Follower {
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Handles one message from the PC; returns the hand-back to send, if control goes back.
    pub async fn handle(&mut self, message: ToMac, desk: &mut dyn MacDesk, replay: &mut dyn Replay) -> Option<HandBack> {
        match message {
            ToMac::Enter { pc_side, along } => {
                self.active = true;
                self.pc_side = pc_side;
                // Mid-drag (a window dragged in the viewer), the pointer is on the window being
                // dragged; moving it would drop the window.
                if !desk.button_held() {
                    desk.warp(landing_on_mac(desk.main(), pc_side, along));
                }
                None
            }
            ToMac::Ping | ToMac::Clip(_) | ToMac::Bye | ToMac::Uses { .. } | ToMac::Latency { .. } => None,
            ToMac::Language(language) => {
                replay.set_language(&language);
                None
            }
            ToMac::Leave => {
                self.active = false;
                replay.release_all().await;
                None
            }
            ToMac::Input(_) if !self.active => None,
            ToMac::Input(Input::Motion { dx, dy }) => {
                let from = desk.pointer();
                let to = step(&desk.displays(), from, (dx, dy));
                let held = desk.button_held();
                desk.move_to(to, (dx, dy), held);
                self.check_hand_back(desk, replay, (from.0 + dx, from.1 + dy)).await
            }
            ToMac::Input(input) => {
                replay.replay(input).await;
                let releases_button = matches!(input, Input::Button { state: 0, .. });
                if releases_button {
                    let here = desk.pointer();
                    self.check_hand_back(desk, replay, here).await
                } else {
                    None
                }
            }
        }
    }

    async fn check_hand_back(
        &mut self,
        desk: &mut dyn MacDesk,
        replay: &mut dyn Replay,
        attempted: (f64, f64),
    ) -> Option<HandBack> {
        let back = hand_back(
            desk.main(),
            desk.virtual_display(),
            self.pc_side,
            desk.pointer(),
            attempted,
            desk.button_held(),
        )?;
        self.active = false;
        replay.release_all().await;
        Some(back)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Final review I4 follow-up: once the Mac connects without Accessibility, the PC captured at
    // the edge and the Mac dropped everything, leaving the user without a mouse.
    #[test]
    fn without_input_replay_the_pointer_is_handed_straight_back() {
        let back = bounce(&ToMac::Enter { pc_side: Side::Left, along: 0.25 }).expect("handed back");
        assert_eq!((back.on_virtual, back.along), (None, 0.25));
        assert_eq!(bounce(&ToMac::Input(Input::Motion { dx: 1.0, dy: 0.0 })), None);
        assert_eq!(bounce(&ToMac::Leave), None);
    }

    const MAIN: Area = Area { left: 0.0, top: 0.0, right: 1728.0, bottom: 1117.0 };
    const VIRTUAL: Area = Area { left: -2560.0, top: -161.0, right: 0.0, bottom: 1279.0 };

    struct Desk {
        pointer: (f64, f64),
        held: bool,
        log: Vec<String>,
        /// A real monitor plugged into the Mac as well, if any.
        external: Option<Area>,
    }

    impl MacDesk for Desk {
        fn main(&self) -> Area {
            MAIN
        }
        fn displays(&self) -> Vec<Area> {
            [Some(MAIN), self.external, Some(VIRTUAL)].into_iter().flatten().collect()
        }
        fn virtual_display(&self) -> Option<Area> {
            Some(VIRTUAL)
        }
        fn pointer(&self) -> (f64, f64) {
            self.pointer
        }
        fn button_held(&self) -> bool {
            self.held
        }
        fn warp(&mut self, to: (f64, f64)) {
            self.log.push(format!("warp {to:?}"));
            self.pointer = to;
        }
        fn move_to(&mut self, to: (f64, f64), _delta: (f64, f64), dragging: bool) {
            self.log.push(format!("{} {to:?}", if dragging { "drag" } else { "move" }));
            self.pointer = to;
        }
    }

    #[derive(Default)]
    struct Keys(Vec<String>);

    #[async_trait]
    impl Replay for Keys {
        async fn replay(&mut self, input: Input) {
            self.0.push(format!("{input:?}"));
        }
        async fn release_all(&mut self) {
            self.0.push("release all".into());
        }
        fn set_language(&mut self, language: &str) {
            self.0.push(format!("language {language}"));
        }
    }

    fn desk(pointer: (f64, f64)) -> Desk {
        Desk { pointer, held: false, log: vec![], external: None }
    }

    async fn enter(f: &mut Follower, d: &mut Desk, k: &mut Keys) {
        assert_eq!(f.handle(ToMac::Enter { pc_side: Side::Left, along: 0.5 }, d, k).await, None);
    }

    fn motion(dx: f64, dy: f64) -> ToMac {
        ToMac::Input(Input::Motion { dx, dy })
    }

    #[tokio::test]
    async fn entering_puts_the_pointer_inside_the_macbooks_edge() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((-900.0, 100.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        assert_eq!(d.pointer, (8.0, 558.5));
        assert!(f.is_active());
    }

    // A window dragged in the viewer and pushed over to the Mac keeps being dragged.
    #[tokio::test]
    async fn entering_mid_drag_leaves_the_pointer_on_the_dragged_window() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((-5.0, 300.0)), Keys::default());
        d.held = true;
        enter(&mut f, &mut d, &mut k).await;
        assert_eq!(d.pointer, (-5.0, 300.0));
        f.handle(motion(20.0, 0.0), &mut d, &mut k).await;
        assert_eq!(d.log.last().unwrap(), "drag (15.0, 300.0)");
    }

    #[tokio::test]
    async fn moves_follow_the_pc_mouse() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        assert_eq!(f.handle(motion(100.0, -50.0), &mut d, &mut k).await, None);
        assert_eq!(d.pointer, (108.0, 508.5));
        assert_eq!(d.log.last().unwrap(), "move (108.0, 508.5)");
    }

    #[tokio::test]
    async fn moving_onto_the_virtual_display_hands_back_and_lets_go_of_keys() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        let back = f.handle(motion(-264.0, 0.5), &mut d, &mut k).await.unwrap();
        assert_eq!(back.on_virtual, Some((0.9, 0.5)));
        assert!(!f.is_active());
        assert_eq!(k.0.last().unwrap(), "release all");
    }

    // Review finding: with a real monitor on the Mac as well, the virtual display was guessed
    // as "the only other display", so control never came back.
    #[tokio::test]
    async fn a_real_monitor_on_the_mac_does_not_hide_the_virtual_display() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        d.external = Some(Area { left: 1728.0, top: 0.0, right: 4288.0, bottom: 1440.0 });
        enter(&mut f, &mut d, &mut k).await;
        let back = f.handle(motion(-264.0, 0.5), &mut d, &mut k).await.unwrap();
        assert_eq!(back.on_virtual, Some((0.9, 0.5)));
    }

    #[tokio::test]
    async fn after_handing_back_moves_are_ignored() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        f.handle(motion(-264.0, 0.5), &mut d, &mut k).await.unwrap();
        let before = d.pointer;
        assert_eq!(f.handle(motion(50.0, 0.0), &mut d, &mut k).await, None);
        assert_eq!(d.pointer, before);
    }

    #[tokio::test]
    async fn letting_go_of_a_window_on_the_virtual_display_hands_back() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        d.held = true;
        assert_eq!(f.handle(motion(-264.0, 0.5), &mut d, &mut k).await, None, "still dragging");
        d.held = false;
        let back = f.handle(ToMac::Input(Input::Button { button: 0x110, state: 0 }), &mut d, &mut k).await;
        assert_eq!(back.unwrap().on_virtual, Some((0.9, 0.5)));
    }

    #[tokio::test]
    async fn the_mac_switches_to_the_pcs_language() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        f.handle(ToMac::Language("he".into()), &mut d, &mut k).await;
        assert_eq!(k.0.last().unwrap(), "language he");
    }

    #[tokio::test]
    async fn keys_and_buttons_are_replayed() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        f.handle(ToMac::Input(Input::Key { key: 30, state: 1 }), &mut d, &mut k).await;
        assert_eq!(k.0, vec!["Key { key: 30, state: 1 }"]);
    }

    #[tokio::test]
    async fn the_pc_taking_control_back_lets_go_of_everything() {
        let (mut f, mut d, mut k) = (Follower::default(), desk((0.0, 0.0)), Keys::default());
        enter(&mut f, &mut d, &mut k).await;
        assert_eq!(f.handle(ToMac::Leave, &mut d, &mut k).await, None);
        assert!(!f.is_active());
        assert_eq!(k.0.last().unwrap(), "release all");
    }
}

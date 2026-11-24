//! Who the Mac lets in: the pure decision, so it is tested on every OS.

use super::channel::{Admit, Refusal};
use crate::mouse::{MouseStatus, Uses};

/// One PC the Mac knows: id, pairing token, allowed, name.
pub type KnownPc = (String, String, bool, String);

/// The entry the PC calling as `id` is checked against: the one with its id, or, when none has
/// it, the single paired entry without an id (a PC migrated from 1.0, whose id the Mac did not
/// trust and learns once the PC proves itself with that entry's token). An empty id is nobody.
pub fn entry<'a>(pcs: &'a [KnownPc], id: &str) -> Option<&'a KnownPc> {
    if id.is_empty() {
        return None;
    }
    if let Some(pc) = pcs.iter().find(|(pc, ..)| pc == id) {
        return Some(pc);
    }
    let mut unverified = pcs.iter().filter(|(pc, token, ..)| pc.is_empty() && !token.is_empty());
    match (unverified.next(), unverified.next()) {
        (Some(pc), None) => Some(pc),
        _ => None,
    }
}

/// Decides whether the PC calling as `id` may use the Mac, given who is connected now
/// (`connected`, a PC id). Unknown and blocked callers are told so before anything about who
/// is connected, so a stranger never learns that.
pub fn decide(pcs: &[KnownPc], connected: Option<&str>, id: &str) -> Admit {
    // A PC without a token was never paired: nobody can prove they are it.
    let Some((_, token, allowed, _)) = entry(pcs, id).filter(|(_, token, ..)| !token.is_empty()) else {
        return Admit::Refuse(Refusal::Unknown);
    };
    if !allowed {
        return Admit::Refuse(Refusal::Blocked);
    }
    if let Some(other) = connected.filter(|c| *c != id) {
        let by = entry(pcs, other).map(|(_, _, _, name)| name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| other.to_string());
        return Admit::Refuse(Refusal::Busy { by });
    }
    Admit::Accept { token: token.clone() }
}

/// The door, before the PC has proved itself. A PC known by its id is answered as [`decide`]
/// answers it. Any id reaches the entry without an id, so a caller matched only that way is just
/// asked to prove itself with that entry's token: whether it is blocked, or who is connected, is
/// told only to a PC that has proved it is that one (by [`decide`] in [`Slot::claim`]).
pub fn door(pcs: &[KnownPc], connected: Option<&str>, id: &str) -> Admit {
    match entry(pcs, id) {
        Some((pc, token, ..)) if pc.is_empty() && !token.is_empty() => Admit::Accept { token: token.clone() },
        _ => decide(pcs, connected, id),
    }
}

/// What the app can tell the Mac's server to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacCommand {
    /// Tell the connected PC to disconnect, and close the connection.
    Disconnect,
    /// Close the connection without a word: the PC takes it for a lost network and connects
    /// again. Its display, on a link of its own, is left alone.
    Restart,
}

/// How the connected PC is let go after a [`MacCommand`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// Told `ToPc::Disconnect` first: it stays disconnected until told otherwise.
    Disconnect,
    /// Dropped without a word, so it reconnects.
    Drop,
}

impl Cut {
    /// What the PC is told before its connection closes, if anything.
    pub fn farewell(self) -> Option<super::wire::ToPc> {
        match self {
            Cut::Disconnect => Some(super::wire::ToPc::Disconnect),
            Cut::Drop => None,
        }
    }
}

/// Who holds the Mac right now, and what they use it for: the serve loop's decisions without
/// any I/O. The door checks a PC before it proves itself, but the slot is claimed only once it
/// has (so knowing a PC's id is not enough to hold the Mac), and before its connection is handed
/// to the loop, so two PCs that prove themselves together cannot both get in.
#[derive(Debug)]
pub struct Slot {
    pcs: Vec<KnownPc>,
    /// The PC holding the slot.
    holder: Option<String>,
    /// Whether the holder's connection has actually arrived (otherwise it is just admitted).
    live: bool,
    /// What the holder said it uses the Mac for; nothing until it says.
    uses: Option<Uses>,
    /// Where the holder's connection came from.
    from: Option<std::net::IpAddr>,
    /// Why the holder's mouse and keyboard cannot be replayed (a missing permission). The
    /// connection stands regardless: the display does not need it.
    problem: Option<String>,
    /// The round trip the holder last reported, in ms.
    rtt_ms: Option<u32>,
}

impl Slot {
    pub fn new(pcs: Vec<KnownPc>) -> Slot {
        Slot { pcs, holder: None, live: false, uses: None, from: None, problem: None, rtt_ms: None }
    }

    /// The PCs the Mac knows changed (one paired, blocked, allowed or forgotten). Whoever holds
    /// the slot keeps it: a blocked or forgotten holder is disconnected by the app's own order.
    pub fn set_pcs(&mut self, pcs: Vec<KnownPc>) {
        self.pcs = pcs;
    }

    /// The door, before the PC has proved itself: whether to go on with it (and its token).
    /// Nothing is claimed.
    pub fn check(&self, id: &str) -> Admit {
        door(&self.pcs, self.holder.as_deref(), id)
    }

    /// The PC has proved itself: it takes the slot, unless another PC took it meanwhile (or it
    /// was blocked or forgotten meanwhile). `Ok(true)` when the claim is new, so a handshake
    /// that then fails should `abandon` it.
    pub fn claim(&mut self, id: &str) -> Result<bool, Refusal> {
        match decide(&self.pcs, self.holder.as_deref(), id) {
            Admit::Accept { .. } => {
                let fresh = self.holder.is_none();
                self.holder = Some(id.to_string());
                Ok(fresh)
            }
            Admit::Refuse(why) => Err(why),
        }
    }

    /// A claimed PC's handshake failed before its connection arrived: give the slot back.
    pub fn abandon(&mut self, id: &str) {
        if !self.live && self.holder.as_deref() == Some(id) {
            self.holder = None;
        }
    }

    /// The PC's connection arrived. Returns true when it replaces a live connection of the
    /// same PC (so whatever that one was doing is over).
    pub fn arrived(&mut self, id: &str) -> bool {
        let replaced = self.live && self.holder.as_deref() == Some(id);
        self.holder = Some(id.to_string());
        self.live = true;
        self.uses = None;
        self.from = None;
        self.rtt_ms = None;
        replaced
    }

    /// The address the holder's connection came from (so the app can keep the PC's address up
    /// to date).
    pub fn came_from(&mut self, ip: std::net::IpAddr) {
        if self.live {
            self.from = Some(ip);
        }
    }

    /// Whether the Mac needs to replay input now: the holder uses the mouse.
    pub fn wants_input(&self) -> bool {
        self.live && self.uses.is_some_and(|u| u.mouse)
    }

    /// Why input cannot be replayed, or `None` once it can.
    pub fn set_problem(&mut self, problem: Option<String>) {
        self.problem = problem;
    }

    /// The holder is gone (said bye, vanished, or was told to disconnect).
    pub fn left(&mut self) {
        self.holder = None;
        self.live = false;
        self.uses = None;
        self.from = None;
        self.rtt_ms = None;
    }

    /// The round trip the holder measured, as it reports it. True when it changed.
    pub fn set_latency(&mut self, ms: u32) -> bool {
        let changed = self.live && self.rtt_ms != Some(ms);
        if changed {
            self.rtt_ms = Some(ms);
        }
        changed
    }

    /// An order from the app: how the connected PC must be let go, or `None` when nobody is
    /// connected. The slot is released here.
    pub fn command(&mut self, command: MacCommand) -> Option<Cut> {
        if !self.live {
            return None;
        }
        self.left();
        Some(match command {
            MacCommand::Disconnect => Cut::Disconnect,
            MacCommand::Restart => Cut::Drop,
        })
    }

    /// The holder's new `Uses`. True when the mouse was on and went off (everything held must
    /// be let go). With nothing in use the PC stops measuring, so the last round trip goes too:
    /// it would otherwise be shown hours later as if current.
    pub fn set_uses(&mut self, now: Uses) -> bool {
        let went_off = self.uses.is_some_and(|u| u.mouse) && !now.mouse;
        if self.live {
            self.uses = Some(now);
            if self.pace().is_idle() {
                self.rtt_ms = None;
            }
        }
        went_off
    }

    /// The heartbeat and silence to keep, from the `Uses` the holder last sent (slow while
    /// nobody is connected or nothing is in use).
    pub fn pace(&self) -> super::wire::Pace {
        super::wire::Pace::of(self.uses.filter(|_| self.live))
    }

    /// Replay of the PC's keys and mouse happens only while it says it uses the mouse.
    pub fn replay_allowed(&self) -> bool {
        self.live && self.uses.is_some_and(|u| u.mouse)
    }

    /// The clipboard is exchanged only while the PC uses both the mouse and the clipboard.
    pub fn clipboard_allowed(&self) -> bool {
        self.live && self.uses.is_some_and(|u| u.mouse && u.clipboard)
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    /// What the app shows: waiting ("connecting") or the connected PC.
    pub fn status(&self) -> MouseStatus {
        match (&self.holder, self.live) {
            (Some(id), true) => {
                let name = entry(&self.pcs, id).map(|p| p.3.clone()).filter(|n| !n.is_empty());
                MouseStatus::Connected {
                    mac_id: id.clone(),
                    mac_name: name.unwrap_or_else(|| id.clone()),
                    uses: self.uses,
                    // Only worth saying while the PC wants its input replayed.
                    problem: self.problem.clone().filter(|_| self.wants_input()),
                    from: self.from,
                    rtt_ms: self.rtt_ms,
                }
            }
            _ => MouseStatus::Connecting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcs() -> Vec<KnownPc> {
        vec![
            ("pc-a".into(), "tok-a".into(), true, "Desk".into()),
            ("pc-b".into(), "tok-b".into(), true, "Laptop".into()),
            ("pc-c".into(), "tok-c".into(), false, "Old".into()),
        ]
    }

    #[test]
    fn an_unknown_pc_is_refused() {
        assert_eq!(decide(&pcs(), None, "pc-x"), Admit::Refuse(Refusal::Unknown));
        // Even while someone is connected, a stranger learns nothing about who.
        assert_eq!(decide(&pcs(), Some("pc-a"), "pc-x"), Admit::Refuse(Refusal::Unknown));
    }

    #[test]
    fn a_blocked_pc_is_refused() {
        assert_eq!(decide(&pcs(), None, "pc-c"), Admit::Refuse(Refusal::Blocked));
        assert_eq!(decide(&pcs(), Some("pc-a"), "pc-c"), Admit::Refuse(Refusal::Blocked));
    }

    #[test]
    fn a_second_pc_is_busy_and_told_who() {
        assert_eq!(decide(&pcs(), Some("pc-a"), "pc-b"), Admit::Refuse(Refusal::Busy { by: "Desk".into() }));
    }

    #[test]
    fn the_same_pc_reconnecting_is_accepted() {
        assert_eq!(decide(&pcs(), Some("pc-a"), "pc-a"), Admit::Accept { token: "tok-a".into() });
        assert_eq!(decide(&pcs(), None, "pc-b"), Admit::Accept { token: "tok-b".into() });
    }

    #[test]
    fn a_pc_with_no_token_is_unknown() {
        let pcs: Vec<KnownPc> = vec![("pc-e".into(), String::new(), true, "Empty".into())];
        assert_eq!(decide(&pcs, None, "pc-e"), Admit::Refuse(Refusal::Unknown));
    }

    // Live check, task 9: a Mac migrated from 1.0 has its PC without a trusted id.
    fn migrated() -> Vec<KnownPc> {
        vec![("pc-a".into(), "tok-a".into(), true, "Desk".into()), (String::new(), "tok-old".into(), true, "Old PC".into())]
    }

    #[test]
    fn an_unknown_caller_may_prove_itself_as_the_pc_without_an_id() {
        assert_eq!(decide(&migrated(), None, "pc-real"), Admit::Accept { token: "tok-old".into() });
        // Without such an entry, it is a stranger.
        assert_eq!(decide(&pcs(), None, "pc-real"), Admit::Refuse(Refusal::Unknown));
    }

    #[test]
    fn a_caller_known_by_id_never_uses_the_pc_without_an_id() {
        assert_eq!(decide(&migrated(), None, "pc-a"), Admit::Accept { token: "tok-a".into() });
        let mut blocked = migrated();
        blocked[0].2 = false;
        assert_eq!(decide(&blocked, None, "pc-a"), Admit::Refuse(Refusal::Blocked));
    }

    #[test]
    fn a_blocked_pc_without_an_id_is_blocked() {
        let mut blocked = migrated();
        blocked[1].2 = false;
        assert_eq!(decide(&blocked, None, "pc-real"), Admit::Refuse(Refusal::Blocked));
    }

    #[test]
    fn two_pcs_without_an_id_are_ambiguous_and_an_empty_id_proves_nothing() {
        let mut two = migrated();
        two.push((String::new(), "tok-other".into(), true, "Other".into()));
        assert_eq!(decide(&two, None, "pc-real"), Admit::Refuse(Refusal::Unknown));
        assert_eq!(decide(&migrated(), None, ""), Admit::Refuse(Refusal::Unknown));
    }

    #[test]
    fn the_pc_without_an_id_is_busy_and_named_like_any_other() {
        let mut slot = Slot::new(migrated());
        assert_eq!(admit(&mut slot, "pc-real"), Ok(true));
        slot.arrived("pc-real");
        assert!(matches!(slot.status(), MouseStatus::Connected { ref mac_id, ref mac_name, .. } if mac_id == "pc-real" && mac_name == "Old PC"));
        assert_eq!(admit(&mut slot, "pc-a"), Err(Refusal::Busy { by: "Old PC".into() }));
        // The app learns its id: the next admission matches by id.
        slot.set_pcs(vec![migrated()[0].clone(), ("pc-real".into(), "tok-old".into(), true, "Old PC".into())]);
        assert_eq!(slot.check("pc-real"), Admit::Accept { token: "tok-old".into() });
        assert!(matches!(slot.status(), MouseStatus::Connected { ref mac_name, .. } if mac_name == "Old PC"));
    }

    // Review of 838ca66, I1: any id reaches the id-less entry, so before its proof a stranger
    // must learn nothing from it: not who is connected, not that it is blocked.
    #[test]
    fn a_stranger_at_the_door_learns_nothing_before_proving_itself() {
        let mut slot = Slot::new(migrated());
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        assert_eq!(slot.check("stranger"), Admit::Accept { token: "tok-old".into() }, "only asked to prove");
        // Proved with the entry's token: it is the migrated PC, and may be told.
        assert_eq!(slot.claim("stranger"), Err(Refusal::Busy { by: "Desk".into() }));
    }

    #[test]
    fn a_blocked_pc_without_an_id_is_told_so_only_after_its_proof() {
        let mut blocked = migrated();
        blocked[1].2 = false;
        let mut slot = Slot::new(blocked);
        assert_eq!(slot.check("stranger"), Admit::Accept { token: "tok-old".into() });
        assert_eq!(slot.claim("stranger"), Err(Refusal::Blocked));
    }

    fn uses(mouse: bool, clipboard: bool) -> Uses {
        Uses { mouse, clipboard, display: false, ctrl_as_cmd: false }
    }

    /// The door's check and the claim after the proof, as the server does them.
    fn admit(slot: &mut Slot, id: &str) -> Result<bool, Refusal> {
        match slot.check(id) {
            Admit::Accept { .. } => slot.claim(id),
            Admit::Refuse(why) => Err(why),
        }
    }

    #[test]
    fn two_pcs_at_the_door_together_the_second_is_busy() {
        let mut slot = Slot::new(pcs());
        // Both pass the door before either has proved itself.
        assert_eq!(slot.check("pc-a"), Admit::Accept { token: "tok-a".into() });
        assert_eq!(slot.check("pc-b"), Admit::Accept { token: "tok-b".into() });
        assert_eq!(slot.claim("pc-a"), Ok(true));
        // pc-a's connection has not reached the loop yet: the loser is told who has it.
        assert_eq!(slot.claim("pc-b"), Err(Refusal::Busy { by: "Desk".into() }));
        assert!(!slot.arrived("pc-a"));
        assert_eq!(slot.check("pc-b"), Admit::Refuse(Refusal::Busy { by: "Desk".into() }));
    }

    // Ledger T6: the slot was claimed before the token proof, so anyone knowing a PC's id could
    // hold the Mac (and get the real PC a sticky Busy).
    #[test]
    fn passing_the_door_without_proving_claims_nothing() {
        let mut slot = Slot::new(pcs());
        assert_eq!(slot.check("pc-a"), Admit::Accept { token: "tok-a".into() });
        // The caller never proved itself: no claim, no abandon needed.
        assert_eq!(admit(&mut slot, "pc-b"), Ok(true));
    }

    #[test]
    fn a_pc_blocked_between_the_door_and_its_proof_is_refused() {
        let mut slot = Slot::new(pcs());
        assert_eq!(slot.check("pc-a"), Admit::Accept { token: "tok-a".into() });
        let mut blocked = pcs();
        blocked[0].2 = false;
        slot.set_pcs(blocked);
        assert_eq!(slot.claim("pc-a"), Err(Refusal::Blocked));
    }

    #[test]
    fn a_failed_handshake_gives_the_slot_back() {
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        slot.abandon("pc-a");
        assert_eq!(admit(&mut slot, "pc-b"), Ok(true));
    }

    #[test]
    fn a_failed_handshake_does_not_drop_a_live_connection() {
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        assert_eq!(admit(&mut slot, "pc-a"), Ok(false), "the same PC again is not a new claim");
        slot.abandon("pc-a");
        assert!(slot.is_live());
        assert_eq!(admit(&mut slot, "pc-b"), Err(Refusal::Busy { by: "Desk".into() }));
    }

    #[test]
    fn the_same_pc_replaces_its_own_connection() {
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        assert!(!slot.arrived("pc-a"));
        slot.set_uses(uses(true, true));
        assert!(slot.arrived("pc-a"), "the old connection is replaced");
        assert!(!slot.replay_allowed(), "the new connection has not said what it uses yet");
    }

    /// The Ctrl as ⌘ setting the Mac reads from its status: what the PC said on this
    /// connection, and nothing new (keep the last) while a new connection has not said yet or
    /// the PC is gone and the Mac waits, so a sharing reconnect keeps it.
    #[test]
    fn ctrl_as_cmd_follows_the_connection_and_holds_through_a_reconnect() {
        use crate::keys_stream::setting_in;
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        slot.set_uses(Uses { ctrl_as_cmd: true, ..uses(false, false) });
        assert_eq!(setting_in(&slot.status()), Some(true));
        slot.set_uses(uses(false, false));
        assert_eq!(setting_in(&slot.status()), Some(false), "turned off");
        slot.set_uses(Uses { ctrl_as_cmd: true, ..uses(false, false) });
        slot.arrived("pc-a");
        assert_eq!(setting_in(&slot.status()), None, "a new connection has not said yet: keep");
        slot.set_uses(Uses { ctrl_as_cmd: true, ..uses(false, false) });
        slot.left();
        assert_eq!(setting_in(&slot.status()), None, "the Mac waits for the PC: keep");
    }

    #[test]
    fn after_a_pc_leaves_the_mac_waits_and_is_free() {
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        assert!(matches!(slot.status(), MouseStatus::Connected { ref mac_name, uses: None, .. } if mac_name == "Desk"));
        slot.left();
        assert_eq!(slot.status(), MouseStatus::Connecting);
        assert_eq!(admit(&mut slot, "pc-b"), Ok(true));
    }

    #[test]
    fn replay_and_clipboard_follow_what_the_pc_uses() {
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        assert!(!slot.replay_allowed() && !slot.clipboard_allowed(), "nothing before Uses");
        assert!(!slot.set_uses(uses(true, false)));
        assert!(slot.replay_allowed() && !slot.clipboard_allowed());
        slot.set_uses(uses(true, true));
        assert!(slot.clipboard_allowed());
        assert!(slot.set_uses(uses(false, true)), "mouse went off");
        assert!(!slot.replay_allowed() && !slot.clipboard_allowed(), "mouse off stops the clipboard too");
        assert!(matches!(slot.status(), MouseStatus::Connected { uses: Some(u), .. } if !u.mouse));
    }

    // Final review C1: pairing a second PC restarted the server and could take the slot from
    // the connected one.
    #[test]
    fn a_pc_paired_while_another_is_connected_leaves_it_connected_and_is_busy() {
        let mut slot = Slot::new(vec![pcs()[0].clone()]);
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        slot.set_uses(uses(true, false));
        slot.set_pcs(pcs());
        assert!(matches!(slot.status(), MouseStatus::Connected { ref mac_id, uses: Some(_), .. } if mac_id == "pc-a"));
        assert!(slot.replay_allowed(), "nothing about the connected PC changed");
        assert_eq!(admit(&mut slot, "pc-b"), Err(Refusal::Busy { by: "Desk".into() }));
    }

    #[test]
    fn a_pc_paired_while_the_mac_waits_is_let_in() {
        let mut slot = Slot::new(vec![pcs()[0].clone()]);
        assert_eq!(admit(&mut slot, "pc-b"), Err(Refusal::Unknown));
        slot.set_pcs(pcs());
        assert_eq!(admit(&mut slot, "pc-b"), Ok(true));
    }

    #[test]
    fn blocking_a_waiting_pc_applies_at_once() {
        let mut slot = Slot::new(pcs());
        let mut changed = pcs();
        changed[1].2 = false;
        slot.set_pcs(changed);
        assert_eq!(admit(&mut slot, "pc-b"), Err(Refusal::Blocked));
    }

    // Final review I4: without Accessibility the PC stays connected (the display does not need
    // it); the missing permission is reported apart, while the PC uses the mouse.
    #[test]
    fn a_problem_with_input_is_reported_without_dropping_the_pc() {
        let mut slot = Slot::new(pcs());
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        slot.set_problem(Some("Accessibility permission is missing".into()));
        slot.set_uses(uses(false, false));
        assert!(!slot.wants_input());
        assert!(matches!(slot.status(), MouseStatus::Connected { problem: None, .. }), "not using the mouse: nothing to say");
        slot.set_uses(uses(true, false));
        assert!(slot.wants_input());
        assert!(matches!(slot.status(), MouseStatus::Connected { problem: Some(ref p), .. } if p.contains("Accessibility")));
        slot.set_problem(None);
        assert!(matches!(slot.status(), MouseStatus::Connected { problem: None, .. }));
    }

    // Final review I5: the app learns where the connected PC is now.
    #[test]
    fn the_connected_pcs_address_is_reported() {
        let ip: std::net::IpAddr = "192.168.1.20".parse().unwrap();
        let mut slot = Slot::new(pcs());
        slot.came_from(ip);
        assert_eq!(slot.status(), MouseStatus::Connecting, "nobody is connected");
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        slot.came_from(ip);
        assert!(matches!(slot.status(), MouseStatus::Connected { from: Some(f), .. } if f == ip));
        slot.left();
        admit(&mut slot, "pc-b").unwrap();
        slot.arrived("pc-b");
        assert!(matches!(slot.status(), MouseStatus::Connected { from: None, .. }));
    }

    #[test]
    fn the_disconnect_command_releases_the_slot() {
        let mut slot = Slot::new(pcs());
        assert_eq!(slot.command(MacCommand::Disconnect), None, "nobody to tell");
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        let cut = slot.command(MacCommand::Disconnect);
        assert_eq!(cut, Some(Cut::Disconnect));
        assert_eq!(cut.unwrap().farewell(), Some(super::super::wire::ToPc::Disconnect));
        assert_eq!(slot.status(), MouseStatus::Connecting);
        assert_eq!(admit(&mut slot, "pc-b"), Ok(true));
    }

    // Plan 4, task 2: Restart lets the PC go without the sticky Disconnect, so it reconnects.
    #[test]
    fn the_restart_command_drops_the_pc_without_a_word() {
        let mut slot = Slot::new(pcs());
        assert_eq!(slot.command(MacCommand::Restart), None, "nobody to drop");
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        let cut = slot.command(MacCommand::Restart);
        assert_eq!(cut, Some(Cut::Drop));
        assert_eq!(cut.unwrap().farewell(), None, "never ToPc::Disconnect");
        assert_eq!(slot.status(), MouseStatus::Connecting);
        // The same PC comes back.
        assert_eq!(admit(&mut slot, "pc-a"), Ok(true));
    }

    #[test]
    fn the_latency_the_pc_reports_is_shown_until_it_leaves() {
        let mut slot = Slot::new(pcs());
        assert!(!slot.set_latency(9), "nobody is connected");
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        assert!(matches!(slot.status(), MouseStatus::Connected { rtt_ms: None, .. }), "not reported yet");
        assert!(slot.set_latency(12));
        assert!(!slot.set_latency(12), "the same value is no change");
        assert!(matches!(slot.status(), MouseStatus::Connected { rtt_ms: Some(12), .. }));
        assert!(slot.arrived("pc-a"));
        assert!(matches!(slot.status(), MouseStatus::Connected { rtt_ms: None, .. }), "a new connection measures anew");
        slot.set_latency(7);
        slot.set_uses(uses(true, false));
        slot.set_latency(8);
        slot.set_uses(uses(false, false));
        assert!(matches!(slot.status(), MouseStatus::Connected { rtt_ms: None, .. }), "nothing in use: not measured");
        slot.left();
        admit(&mut slot, "pc-b").unwrap();
        slot.arrived("pc-b");
        assert!(matches!(slot.status(), MouseStatus::Connected { rtt_ms: None, .. }));
    }

    #[test]
    fn the_macs_pace_follows_the_uses_it_last_heard() {
        use crate::share::wire::Pace;
        let mut slot = Slot::new(pcs());
        assert_eq!(slot.pace(), Pace::IDLE, "nobody connected: nothing to watch closely");
        admit(&mut slot, "pc-a").unwrap();
        slot.arrived("pc-a");
        assert_eq!(slot.pace(), Pace::IDLE, "nothing said yet");
        slot.set_uses(Uses { mouse: false, clipboard: false, display: true, ctrl_as_cmd: false });
        assert_eq!(slot.pace(), Pace::BUSY, "the display");
        slot.set_uses(uses(true, true));
        assert_eq!(slot.pace(), Pace::BUSY, "the mouse");
        slot.set_uses(uses(false, false));
        assert_eq!(slot.pace(), Pace::IDLE, "all off");
        slot.set_uses(uses(true, false));
        slot.left();
        assert_eq!(slot.pace(), Pace::IDLE, "gone");
    }
}

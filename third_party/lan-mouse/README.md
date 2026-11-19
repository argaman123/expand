# Lan Mouse input crates (vendored)

`input-capture`, `input-emulation` and `input-event` from Lan Mouse
(https://github.com/feschber/lan-mouse) at commit 69958c6, GPL-3.0 (see LICENSE).
Sidescreen uses them as libraries for capturing input on the PC and replaying it on the Mac;
its own code (crates/core/src/share) does the coordination and the network part.

Changed here, in `input-capture/src/windows` (each change marked "Sidescreen:"):
- `patches/lan-mouse-windows-edge.patch`: pushing on against the edge crosses again after a
  released hop.
- Dropping a capture no longer panics: the hook thread stops before its queue goes, and
  messages to a thread that has already ended are ignored (2026-11-18, found when a session
  ended after the Mac stopped answering).
- The capture thread creates its message queue before publishing its id, so the request to arm
  the screen edge is never lost when a new capture starts (2026-11-18).

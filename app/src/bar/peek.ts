// What of the bar shows, and when it shows by itself (a "peek"): for a moment after it comes up
// over the full-screen display (it went full screen, or came back in front), so it is clear where
// the bar is. There is no tab step (round 3): near the top centre, the whole bar shows. Pure, for
// the tests.
import type { BarShape } from "../api";

/** How long the bar shows by itself (ms), before it goes unless the pointer is near. */
export const PEEK_MS = 3000;

/** What decides what of the bar shows. */
export interface BarState {
  /** The window holds the display window's title bar settings popover (nothing else shows). */
  popup: boolean;
  /** That popover is open. */
  popOpen: boolean;
  /** The bar is open (the pointer on it, or a menu or popover of its own open). */
  open: boolean;
  /** A menu or popover of the bar is open (all of the window shows). */
  menu: boolean;
  /** The pointer is near the top centre (the app tells). */
  near: boolean;
  /** The bar shows by itself for a moment. */
  peeking: boolean;
}

/** What shows: the bar while open, near or peeking (all of the window with a menu or popover),
 *  else nothing. Holding the title bar's popover, only that. */
export function barTarget(s: BarState): BarShape {
  if (s.popup) return s.popOpen ? "menu" : "hidden";
  if (s.open && s.menu) return "menu";
  return s.open || s.near || s.peeking ? "bar" : "hidden";
}

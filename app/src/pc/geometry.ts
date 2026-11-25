import type { Monitor, Placement } from "../api";

/** Between neighbouring monitors, and between the MacBook and the monitor next to it (stage px). */
const GAP = 16;
const SIDE_GAP = 24;
/** Kept clear round the arrangement, inside the stage, so nothing touches its edge. */
const PAD = 16;
/** How much of the room the arrangement takes (round 4, the owner: a little smaller, with air round it). */
export const DESK_FILL = 0.86;
/** The largest scale: inches to stage pixels. */
const MAX_K = 22;
/** The MacBook drawn as a 13-inch one (the owner's pick, round 3): the drawing's width in inches
 *  (the base, wider than the lid), and its height to width (a 16:10 lid on a simple base). */
const MAC_W_IN = 10.4;
const MAC_ASPECT = 0.64;
/** The lid's screen as a share of the drawing's width (pc.css: the lid is 94% wide, its screen
 *  inset 1.7% of the drawing's width on each side): about 9.4 inches, two fifths of a 27-inch
 *  monitor's width. */
export const LID_SCREEN_SHARE = 0.906;
/** The stand's height, from the floor to the screens' bottom, as a share of the tallest screen's
 *  height: the one knob for how high the monitors stand (pc.css `--ex-stand-neck` overrides it, for
 *  trying values). A MacBook under a monitor stands in front of it. */
export const STAND_NECK = 0.19;
/** The drawing's proportions, each overridable (Desk.tsx reads pc.css knobs for them, to try values):
 *  - `neck`: STAND_NECK;
 *  - `macW`, `macAspect`: MAC_W_IN, MAC_ASPECT;
 *  - `neckW`: the stand's upright, as a share of its screen's width;
 *  - `baseW`: its foot, as a share of the screen's width (never narrower than the upright, nor than
 *    the upright and 16 px at a tiny scale);
 *  - `footH`: the foot's height, as a share of the stand's. */
export type DeskShape = { neck: number; macW: number; macAspect: number; neckW: number; baseW: number; footH: number };
export const DESK_SHAPE: DeskShape = { neck: STAND_NECK, macW: MAC_W_IN, macAspect: MAC_ASPECT, neckW: 0.19, baseW: 0.43, footH: 0.2 };

/** A screen raised less than this above the lowest one stands on the same stand height (stage px). */
const RAISE_MIN = 4;
/** How far the floor line reaches past the arrangement on each side (stage px). */
const FLOOR_OVER = 24;

export type Box = { w: number; h: number };
export type Rect = { x: number; y: number; w: number; h: number };
export type DeskMonitor = Rect & { number: number; neck: Rect; base: Rect };
export type Slot = { id: string; x: number; y: number; side: "left" | "right" | "bottom"; monitor: number | null };
export type DeskLayout = {
  monitors: DeskMonitor[];
  slots: Slot[];
  /** The floor: the bottom of every stand's base and of the MacBook. */
  deskY: number;
  mac: { w: number; h: number };
  /** The floor line under the arrangement, reaching a little past it on both sides. */
  floor: { x: number; w: number };
};

const empty = (box: Box): DeskLayout => ({ monitors: [], slots: [], deskY: Math.round(box.h / 2), mac: { w: 0, h: 0 }, floor: { x: 0, w: 0 } });

/** Pixels per inch, guessed from the resolution (Windows gives no physical size here): 1080 rows
 *  is a 24-inch monitor, 1440 (or up to 1600) a 27-inch one, more is a 4K monitor of about 30. */
function ppiFor(h: number): number {
  if (h <= 1200) return 92;
  if (h <= 1600) return 109;
  return 143;
}

/** A monitor's screen in inches, from its size in pixels. */
export function monitorInches(w: number, h: number): Box {
  const ppi = ppiFor(Math.min(w, h));
  return { w: w / ppi, h: h / ppi };
}

/** Left to right as they really stand (then top to bottom). */
function byPosition(monitors: Monitor[]): Monitor[] {
  return [...monitors].sort((a, b) => a.left - b.left || a.top - b.top || a.number - b.number);
}

/** The place's id for a placement, given the monitors' numbers left to right: `top` falls back to
 *  right, a vanished monitor to the leftmost. */
function slotIdFor(placement: Placement, numbers: number[]): string | null {
  if (numbers.length === 0) return null;
  if (placement.side === "left") return "left";
  if (placement.side === "bottom") {
    const n = placement.monitor != null && numbers.includes(placement.monitor) ? placement.monitor : numbers[0];
    return `under-${n}`;
  }
  return "right";
}

/** The place a backend placement stands for, before there is a layout. */
export function placementSlotId(placement: Placement, monitors: Monitor[]): string | null {
  return slotIdFor(placement, byPosition(monitors).map((m) => m.number));
}

/** The arrangement drawn to scale: the real monitors side by side in their real order and sizes,
 *  each on a low stand, every stand and the MacBook on one floor. The whole arrangement, the MacBook
 *  at place `at` included, is centred in the stage and as large as the stage allows wherever the
 *  MacBook is. While it is dragged, `dragView` zooms out to show every place. */
export function layoutDesk(monitors: Monitor[], box: Box, at: string | null, shape: Partial<DeskShape> = {}): DeskLayout {
  const { neck: standNeck, macW: macWIn, macAspect, neckW: neckShare, baseW: baseShare, footH } = { ...DESK_SHAPE, ...shape };
  const list = byPosition(monitors);
  if (list.length === 0 || box.w <= 0 || box.h <= 0) return empty(box);
  // In inches. A screen raised above the lowest one (in the desktop's pixels) is raised by as many
  // of its own pixels.
  const lowest = Math.max(...list.map((m) => m.bottom));
  const real = list.map((m) => {
    const px = { w: Math.max(1, m.right - m.left), h: Math.max(1, m.bottom - m.top) };
    const inches = monitorInches(px.w, px.h);
    const perPx = inches.w / px.w;
    return { number: m.number, w: inches.w, h: inches.h, raise: Math.min((lowest - m.bottom) * perPx, inches.h / 2) };
  });
  const sumW = real.reduce((s, d) => s + d.w, 0);
  const raiseOf = (d: (typeof real)[number]) => d.raise;
  const gaps = GAP * (real.length - 1);

  // Everything but the gaps grows with the scale k.
  const macW1 = macWIn; // at k = 1
  const macH1 = macW1 * macAspect;
  const stand1 = standNeck * Math.max(...real.map((d) => d.h));
  const tallest1 = Math.max(...real.map((d) => d.h + raiseOf(d)));
  // Wide enough for the monitors and a MacBook beside them; tall enough for the highest screen.
  const kW = (box.w - 2 * PAD - gaps - SIDE_GAP) / (sumW + macW1);
  const kH = (box.h - 2 * PAD) / Math.max(tallest1 + stand1, macH1);
  const k = Math.max(0.0001, Math.min(kW, kH, MAX_K) * DESK_FILL);

  const macW = Math.round(macW1 * k);
  const macH = Math.round(macH1 * k);
  const stand = Math.round(stand1 * k);

  // Laid out with the floor at y = 0 and the leftmost screen at x = 0, then moved to the centre.
  let x = 0;
  const mons = real.map((d) => {
    const w = Math.round(d.w * k);
    const h = Math.round(d.h * k);
    const raise = Math.round(raiseOf(d) * k);
    const m = { number: d.number, x, y: -stand - (raise < RAISE_MIN ? 0 : raise) - h, w, h };
    x += w + GAP;
    return m;
  });
  const groupW = x - GAP;
  const slots: Slot[] = [
    { id: "left", x: -SIDE_GAP - macW, y: -macH, side: "left", monitor: null },
    { id: "right", x: groupW + SIDE_GAP, y: -macH, side: "right", monitor: null },
    ...mons.map<Slot>((m) => ({ id: `under-${m.number}`, x: Math.round(m.x + m.w / 2 - macW / 2), y: -macH, side: "bottom", monitor: m.number })),
  ];

  const here = slots.find((s) => s.id === at);
  const minX = Math.min(0, here ? here.x : 0);
  const maxX = Math.max(groupW, here ? here.x + macW : 0);
  const minY = Math.min(...mons.map((m) => m.y));
  const ox = Math.round((box.w - (maxX - minX)) / 2 - minX);
  const oy = Math.round((box.h - minY) / 2); // the floor (y = 0) lands here: minY is the top, above it

  return {
    monitors: mons.map((m) => {
      const bottom = m.y + m.h;
      const neckW = Math.max(8, Math.round(m.w * neckShare));
      const baseW = Math.max(neckW + 16, Math.round(m.w * baseShare));
      const baseH = Math.max(2, Math.round(stand * footH));
      const cx = m.x + m.w / 2;
      return {
        number: m.number,
        x: m.x + ox,
        y: m.y + oy,
        w: m.w,
        h: m.h,
        // The neck starts a little behind the screen and reaches into the base.
        neck: { x: Math.round(cx - neckW / 2) + ox, y: bottom - 4 + oy, w: neckW, h: -bottom + 4 - baseH + 1 },
        base: { x: Math.round(cx - baseW / 2) + ox, y: -baseH + oy, w: baseW, h: baseH },
      };
    }),
    slots: slots.map((s) => ({ ...s, x: s.x + ox, y: s.y + oy })),
    deskY: oy,
    mac: { w: macW, h: macH },
    floor: { x: minX - FLOOR_OVER + ox, w: maxX - minX + 2 * FLOOR_OVER },
  };
}

/** The glow at the edge the cursor crosses: leftmost monitor's left, rightmost's right, or a monitor's bottom. */
export function edgeFor(slot: Slot, layout: DeskLayout): Rect | null {
  const mons = layout.monitors;
  if (mons.length === 0) return null;
  if (slot.side === "left") {
    const m = mons[0];
    return { x: m.x, y: m.y + 2, w: 3, h: m.h - 4 };
  }
  if (slot.side === "right") {
    const m = mons[mons.length - 1];
    return { x: m.x + m.w - 3, y: m.y + 2, w: 3, h: m.h - 4 };
  }
  const m = mons.find((x) => x.number === slot.monitor) ?? mons[0];
  return { x: m.x + 2, y: m.y + m.h - 3, w: m.w - 4, h: 3 };
}

/** The place a backend placement stands for in a layout. */
export function slotFor(placement: Placement, layout: DeskLayout): Slot | null {
  const id = slotIdFor(placement, layout.monitors.map((m) => m.number));
  return layout.slots.find((s) => s.id === id) ?? null;
}

/** The place nearest the dragged MacBook (x, y: its top-left; every place is the MacBook's size). */
export function nearestSlot(layout: DeskLayout, x: number, y: number): Slot | null {
  let best: Slot | null = null;
  let dist = Infinity;
  for (const s of layout.slots) {
    const d = Math.hypot(s.x - x, s.y - y);
    if (d < dist) {
      dist = d;
      best = s;
    }
  }
  return best;
}

/** The monitor the Mac display covers: the backend's choice (`viewer_monitor`); the first monitor if it has none. */
export function overlayMonitor(monitors: Monitor[], viewer: number | null | undefined): number | null {
  if (monitors.length === 0) return null;
  return viewer != null && monitors.some((m) => m.number === viewer) ? viewer : monitors[0].number;
}

/** The view while the MacBook is dragged: the layout zoomed out (never in) round the middle of the
 *  stage so that every place, and every monitor, shows. Stage position = (x, y) + s × layout position. */
export function dragView(layout: DeskLayout, box: Box): { s: number; x: number; y: number } {
  const rects: Rect[] = [...layout.monitors, ...layout.monitors.map((m) => m.base), ...layout.slots.map((p) => ({ x: p.x, y: p.y, w: layout.mac.w, h: layout.mac.h }))];
  if (rects.length === 0) return { s: 1, x: 0, y: 0 };
  const left = Math.min(...rects.map((r) => r.x));
  const right = Math.max(...rects.map((r) => r.x + r.w));
  const top = Math.min(...rects.map((r) => r.y));
  const bottom = Math.max(...rects.map((r) => r.y + r.h));
  const s = Math.min(1, (box.w - 2 * PAD) / (right - left), (box.h - 2 * PAD) / (bottom - top));
  if (s >= 1 && left >= PAD && right <= box.w - PAD) return { s: 1, x: 0, y: 0 };
  return { s, x: (box.w - s * (right - left)) / 2 - s * left, y: (box.h - s * (bottom - top)) / 2 - s * top };
}

/** A rect on the desktop, in physical pixels (the window's, a monitor's). */
export type ScreenRect = { left: number; top: number; right: number; bottom: number };
/** The Mac's light in the window: the window's side it comes in from, how strong (0..1) and where
 *  along that side its middle falls (0..1, from the top, or the left for "bottom"). */
export type Heat = { side: "left" | "right" | "bottom"; strength: number; along: number };
/** Within this share of the edge monitor's height a window counts as touching an edge under it
 *  (a maximised window stops at the taskbar, a tenth of a 1440-row monitor at 300%). A side edge
 *  has no taskbar in the way: there, only a window that touches it is full (round 4: a slight move
 *  off it should show). The light is gone at the monitor's whole size. */
const HEAT_NEAR = 0.11;
/** How fast the light falls off with the window's distance from the edge (as a share of the
 *  monitor's size): steep near the edge, so nudging the window shows, with a long faint tail. */
const HEAT_FALL = 4;

/** The Mac's light for a window at `win`: it comes in from the side of the crossing edge (as the
 *  desk draws it: the leftmost monitor's left, the rightmost's right, or the bottom of the one the
 *  MacBook is under), full when the window touches the edge or overhangs it, fading with the
 *  window's distance from it to nothing at the edge monitor's width (its height, for an edge
 *  under it). The window is first cut to the monitor its middle is on: Windows' invisible frame
 *  makes a maximised window's rect overhang its monitor, by more at a higher scale. It must then
 *  reach along the edge (overlap its span) to have any: one beside the edge, as on the monitor
 *  next to the one the MacBook is under, has none. So a window on a monitor with no crossing edge
 *  has none. */
export function heatFor(win: ScreenRect, monitors: Monitor[], placement: Placement): Heat | null {
  const list = byPosition(monitors);
  const id = slotIdFor(placement, list.map((m) => m.number));
  if (!id) return null;
  const clamp = (v: number) => Math.min(1, Math.max(0, v));
  // The edge as a segment: fixed at `at` on one axis, running lo..hi on the other.
  let side: Heat["side"], at: number, lo: number, hi: number, reach: number;
  if (id === "left" || id === "right") {
    const m = id === "left" ? list[0] : list[list.length - 1];
    [side, at, lo, hi, reach] = [id, id === "left" ? m.left : m.right, m.top, m.bottom, m.right - m.left];
  } else {
    const m = list.find((x) => `under-${x.number}` === id) ?? list[0];
    [side, at, lo, hi, reach] = ["bottom", m.bottom, m.left, m.right, m.bottom - m.top];
  }
  const vertical = side !== "bottom";
  // The window cut to the monitor its middle is on (none: as it is).
  const cx = (win.left + win.right) / 2;
  const cy = (win.top + win.bottom) / 2;
  const home = list.find((m) => cx >= m.left && cx < m.right && cy >= m.top && cy < m.bottom);
  const w = home
    ? { left: Math.max(win.left, home.left), top: Math.max(win.top, home.top), right: Math.min(win.right, home.right), bottom: Math.min(win.bottom, home.bottom) }
    : win;
  // The window's extent across the edge (a, b) and along it (from, to).
  const [a, b] = vertical ? [w.left, w.right] : [w.top, w.bottom];
  const [from, to] = vertical ? [w.top, w.bottom] : [w.left, w.right];
  const across = Math.max(0, a - at, at - b);
  const near = vertical ? 0 : HEAT_NEAR * reach;
  // Beside the edge, not across from it (meeting it only at a corner counts as beside): none.
  const beside = to <= lo || from >= hi;
  const t = beside ? 1 : clamp((across - near) / Math.max(1, reach - near));
  // The middle of where the window and the edge overlap, or the edge's nearest end, as a share of
  // the whole window's side (as the page draws it).
  const mid = to < lo ? lo : from > hi ? hi : (Math.max(from, lo) + Math.min(to, hi)) / 2;
  const [wFrom, wTo] = vertical ? [win.top, win.bottom] : [win.left, win.right];
  const strength = (Math.exp(-HEAT_FALL * t) - Math.exp(-HEAT_FALL)) / (1 - Math.exp(-HEAT_FALL));
  return { side, strength: clamp(strength), along: clamp((mid - wFrom) / Math.max(1, wTo - wFrom)) };
}

// The desk picture's geometry. Run with `npm test` (Node's own runner, which strips the types).
import { test } from "node:test";
import assert from "node:assert/strict";
import { DESK_SHAPE, LID_SCREEN_SHARE, STAND_NECK, dragView, edgeFor, heatFor, layoutDesk, monitorInches, nearestSlot, placementSlotId, slotFor, type DeskLayout } from "../src/pc/geometry.ts";

// The owner's desk: monitor 1 (1920 × 1080) left of monitor 2 (2560 × 1440), bottoms 2 px apart.
const DESK = [
  { number: 2, left: 0, top: 0, right: 2560, bottom: 1440 },
  { number: 1, left: -1920, top: 358, right: 0, bottom: 1438 },
];
// Stages: the default window, the minimum window, and the minimum window under a banner.
const STAGES = [
  { w: 1160, h: 440 },
  { w: 880, h: 300 },
  { w: 880, h: 260 },
  { w: 1840, h: 760 },
];
const PLACES = ["left", "right", "under-1", "under-2"];

/** The arrangement with the MacBook at `at`: monitors, stands and the MacBook. */
function bounds(l: DeskLayout, at: string) {
  const mac = l.slots.find((s) => s.id === at)!;
  const rects = [...l.monitors, ...l.monitors.map((m) => m.base), { x: mac.x, y: mac.y, w: l.mac.w, h: l.mac.h }];
  return {
    left: Math.min(...rects.map((r) => r.x)),
    right: Math.max(...rects.map((r) => r.x + r.w)),
    top: Math.min(...rects.map((r) => r.y)),
    bottom: Math.max(...rects.map((r) => r.y + r.h)),
  };
}

for (const box of STAGES) {
  for (const at of PLACES) {
    test(`${box.w}×${box.h}, MacBook ${at}: the arrangement is centred`, () => {
      const l = layoutDesk(DESK, box, at);
      const b = bounds(l, at);
      assert.ok(Math.abs((b.left + b.right) / 2 - box.w / 2) <= 1, `horizontal centre ${(b.left + b.right) / 2} vs ${box.w / 2}`);
      assert.ok(Math.abs((b.top + b.bottom) / 2 - box.h / 2) <= 1, `vertical centre ${(b.top + b.bottom) / 2} vs ${box.h / 2}`);
      assert.ok(Math.abs(l.floor.x + l.floor.w / 2 - box.w / 2) <= 1, "the floor line is centred too");
    });

    test(`${box.w}×${box.h}, MacBook ${at}: the arrangement fits the stage`, () => {
      const l = layoutDesk(DESK, box, at);
      const b = bounds(l, at);
      assert.ok(b.left >= 0 && b.right <= box.w && b.top >= 0 && b.bottom <= box.h, JSON.stringify(b));
    });

    test(`${box.w}×${box.h}, MacBook ${at}: while dragging, the zoomed-out view shows every place`, () => {
      const l = layoutDesk(DESK, box, at);
      const v = dragView(l, box);
      assert.ok(v.s > 0 && v.s <= 1);
      const sx = (x: number) => v.x + v.s * x;
      const sy = (y: number) => v.y + v.s * y;
      for (const s of l.slots) {
        assert.ok(sx(s.x) >= 0 && sx(s.x + l.mac.w) <= box.w, `${s.id} x ${sx(s.x)}..${sx(s.x + l.mac.w)}`);
        assert.ok(sy(s.y) >= 0 && sy(s.y + l.mac.h) <= box.h, `${s.id} y`);
      }
      for (const m of l.monitors) assert.ok(sx(m.x) >= 0 && sx(m.x + m.w) <= box.w && sy(m.y) >= 0, `monitor ${m.number}`);
    });
  }

  test(`${box.w}×${box.h}: one floor under every stand and every place`, () => {
    const l = layoutDesk(DESK, box, "right");
    for (const m of l.monitors) {
      assert.equal(m.base.y + m.base.h, l.deskY, `base of ${m.number}`);
      assert.ok(m.neck.y <= m.y + m.h && m.neck.y + m.neck.h >= m.base.y, `neck of ${m.number} joins screen and base`);
      // Stands are centred under their screens.
      assert.ok(Math.abs(m.neck.x + m.neck.w / 2 - (m.x + m.w / 2)) <= 1);
      assert.ok(Math.abs(m.base.x + m.base.w / 2 - (m.x + m.w / 2)) <= 1);
    }
    for (const s of l.slots) assert.equal(s.y + l.mac.h, l.deskY, `${s.id} on the floor`);
  });

  test(`${box.w}×${box.h}: a MacBook under a monitor stands in front of it, centred`, () => {
    const l = layoutDesk(DESK, box, "under-2");
    for (const m of l.monitors) {
      const s = l.slots.find((x) => x.id === `under-${m.number}`)!;
      assert.ok(Math.abs(s.x + l.mac.w / 2 - (m.x + m.w / 2)) <= 1, "centred under the monitor");
      assert.ok(s.x > m.x && s.x + l.mac.w < m.x + m.w, "narrower than the monitor: its bottom edge shows on both sides");
    }
  });

  test(`${box.w}×${box.h}: the stands: STAND_NECK high, a slim post on a wide, low plate`, () => {
    const l = layoutDesk(DESK, box, "right");
    const tallest = Math.max(...l.monitors.map((m) => m.h));
    for (const m of l.monitors) {
      const gap = l.deskY - (m.y + m.h); // screen bottom to the floor
      assert.ok(Math.abs(gap - STAND_NECK * tallest) <= 2, `stand ${gap} of ${tallest}`);
      assert.ok(m.base.h <= gap / 3, "a low plate");
    }
  });
}

test("monitor sizes in inches: about 27 inches for 2560 × 1440, 24 for 1920 × 1080", () => {
  const diag = (w: number, h: number) => {
    const i = monitorInches(w, h);
    return Math.hypot(i.w, i.h);
  };
  assert.ok(Math.abs(diag(2560, 1440) - 27) < 0.6, `${diag(2560, 1440)}`);
  assert.ok(Math.abs(diag(1920, 1080) - 24) < 0.6, `${diag(1920, 1080)}`);
  assert.ok(Math.abs(diag(1920, 1200) - 24) < 1.2, `${diag(1920, 1200)}`);
  assert.ok(Math.abs(diag(3440, 1440) - 34) < 1, `${diag(3440, 1440)}`);
  assert.ok(diag(3840, 2160) >= 27 && diag(3840, 2160) <= 32.5, `${diag(3840, 2160)}`);
});

for (const box of STAGES) {
  test(`${box.w}×${box.h}: the MacBook's screen is about two fifths of a 27-inch monitor's width`, () => {
    const l = layoutDesk(DESK, box, "right");
    const big = l.monitors.find((m) => m.number === 2)!;
    const share = (l.mac.w * LID_SCREEN_SHARE) / big.w;
    assert.ok(share > 0.36 && share < 0.45, `${share}`);
  });
  test(`${box.w}×${box.h}: each stand is a wide back plate on a wider foot`, () => {
    const l = layoutDesk(DESK, box, "right");
    for (const m of l.monitors) {
      assert.ok(m.neck.w / m.w > 0.15 && m.neck.w / m.w < 0.24, `back plate ${m.neck.w / m.w}`);
      assert.ok(m.base.w / m.w > 0.4 && m.base.w / m.w < 0.46, `foot ${m.base.w / m.w}`);
      assert.ok(m.base.w > m.neck.w);
    }
  });
}

test("the default window: the largest monitor is about a third of the window's width", () => {
  // The window is 1240 wide; the stage is the window less 40 px on each side.
  const l = layoutDesk(DESK, { w: 1160, h: 440 }, "right");
  const big = Math.max(...l.monitors.map((m) => m.w));
  // With the 13-inch MacBook (round 3) the monitors take a little more of it.
  assert.ok(big / 1240 >= 0.3 && big / 1240 <= 0.39, `${big} px`);
});

test("the stand knob: a higher stand raises the screens, the floor stays under everything", () => {
  const box = { w: 1160, h: 440 };
  const low = layoutDesk(DESK, box, "right", { neck: 0.09 });
  const high = layoutDesk(DESK, box, "right", { neck: 0.25 });
  for (const l of [low, high]) for (const m of l.monitors) assert.equal(m.base.y + m.base.h, l.deskY);
  const gap = (l: DeskLayout) => l.deskY - (l.monitors[1].y + l.monitors[1].h);
  assert.ok(gap(high) > gap(low) * 2);
});

test("the shape knobs: the MacBook's size and the stand's parts, with today's look by default", () => {
  const box = { w: 1160, h: 440 };
  const base = layoutDesk(DESK, box, "right");
  assert.deepEqual(layoutDesk(DESK, box, "right", {}), base);
  assert.deepEqual(layoutDesk(DESK, box, "right", { ...DESK_SHAPE }), base);
  // A smaller, slimmer MacBook: narrower and less tall, on the same floor.
  const small = layoutDesk(DESK, box, "right", { macW: 10, macAspect: 0.6 });
  assert.ok(small.mac.w < base.mac.w);
  assert.ok(Math.abs(small.mac.h / small.mac.w - 0.6) < 0.02);
  const mac = small.slots.find((s) => s.id === "right")!;
  assert.equal(mac.y + small.mac.h, small.deskY);
  // A one-piece stand: a wide slab on a thin foot, every foot on the floor, the slab at least the foot's width.
  const slab = layoutDesk(DESK, box, "right", { neckW: 0.36, baseW: 0.4, footH: 0.08 });
  for (const m of slab.monitors) {
    assert.ok(Math.abs(m.neck.w / m.w - 0.36) < 0.02, `${m.neck.w / m.w}`);
    assert.ok(m.base.w >= m.neck.w);
    assert.equal(m.base.y + m.base.h, slab.deskY);
    assert.ok(m.base.h < base.monitors[0].base.h);
    assert.ok(Math.abs(m.neck.x + m.neck.w / 2 - (m.x + m.w / 2)) <= 1, "centred under the screen");
  }
});

test("real sizes and order: monitor 1 left of monitor 2, in physical proportion, screen bottoms level", () => {
  const l = layoutDesk(DESK, { w: 1160, h: 440 }, "right");
  const [a, b] = l.monitors;
  assert.deepEqual([a.number, b.number], [1, 2]);
  assert.ok(a.x + a.w < b.x);
  // A 24-inch monitor next to a 27-inch one, not 1920 px next to 2560 px.
  const physical = monitorInches(1920, 1080).w / monitorInches(2560, 1440).w;
  assert.ok(Math.abs(a.w / b.w - physical) < 0.02, `${a.w / b.w} vs ${physical}`);
  assert.ok(Math.abs(a.h / a.w - 1080 / 1920) < 0.02);
  assert.equal(a.y + a.h, b.y + b.h, "2 real px apart reads as level");
});

test("a raised monitor stands higher, on the same floor", () => {
  const raised = [DESK[0], { ...DESK[1], top: 0, bottom: 1080 }];
  const l = layoutDesk(raised, { w: 1160, h: 440 }, "right");
  const [a, b] = l.monitors;
  assert.ok(a.y + a.h < b.y + b.h - 4);
  assert.equal(a.base.y + a.base.h, l.deskY);
});

test("the arrangement moves with the MacBook, the monitors keep their size", () => {
  const box = { w: 1160, h: 440 };
  const left = layoutDesk(DESK, box, "left");
  const right = layoutDesk(DESK, box, "right");
  const under = layoutDesk(DESK, box, "under-1");
  assert.ok(left.monitors[0].x > under.monitors[0].x && under.monitors[0].x > right.monitors[0].x);
  assert.equal(left.monitors[1].w, right.monitors[1].w);
  assert.equal(left.deskY, right.deskY);
});

test("whole pixels everywhere", () => {
  const l = layoutDesk(DESK, { w: 1013, h: 377 }, "under-1");
  const nums = [l.deskY, l.mac.w, l.mac.h, ...l.monitors.flatMap((m) => [m.x, m.y, m.w, m.h, m.neck.x, m.neck.y, m.neck.h, m.base.x, m.base.w]), ...l.slots.flatMap((s) => [s.x, s.y])];
  for (const n of nums) assert.ok(Number.isInteger(n), `${n}`);
});

test("one monitor, and none", () => {
  const one = layoutDesk([DESK[0]], { w: 880, h: 300 }, "under-2");
  assert.equal(one.monitors.length, 1);
  assert.deepEqual(one.slots.map((s) => s.id), ["left", "right", "under-2"]);
  const none = layoutDesk([], { w: 880, h: 300 }, "right");
  assert.equal(none.monitors.length, 0);
  assert.equal(slotFor({ side: "right", monitor: null }, none), null);
});

test("placements map to places", () => {
  assert.equal(placementSlotId({ side: "left", monitor: null }, DESK), "left");
  assert.equal(placementSlotId({ side: "top", monitor: null }, DESK), "right");
  assert.equal(placementSlotId({ side: "bottom", monitor: 2 }, DESK), "under-2");
  assert.equal(placementSlotId({ side: "bottom", monitor: 7 }, DESK), "under-1", "a vanished monitor: the leftmost");
  assert.equal(placementSlotId({ side: "right", monitor: null }, []), null);
  const l = layoutDesk(DESK, { w: 1160, h: 440 }, "right");
  assert.equal(slotFor({ side: "bottom", monitor: 2 }, l)?.id, "under-2");
});

test("dropping snaps to the nearest place", () => {
  const l = layoutDesk(DESK, { w: 1160, h: 440 }, "right");
  const under1 = l.slots.find((s) => s.id === "under-1")!;
  assert.equal(nearestSlot(l, under1.x + 30, under1.y - 20)?.id, "under-1");
  const left = l.slots.find((s) => s.id === "left")!;
  assert.equal(nearestSlot(l, left.x - 40, left.y - 60)?.id, "left");
});

test("the crossing edge follows the place", () => {
  const l = layoutDesk(DESK, { w: 1160, h: 440 }, "right");
  const [m1, m2] = l.monitors;
  assert.equal(edgeFor(l.slots.find((s) => s.id === "left")!, l)?.x, m1.x);
  assert.equal(edgeFor(l.slots.find((s) => s.id === "right")!, l)?.x, m2.x + m2.w - 3);
  assert.equal(edgeFor(l.slots.find((s) => s.id === "under-2")!, l)?.y, m2.y + m2.h - 3);
});

// The light that knows where the Mac is: from the window's place on the desktop (physical px).
// DESK: monitor 1 (1920 × 1080, at x -1920, y 358) left of monitor 2 (2560 × 1440, at 0, 0).
const RIGHT = { side: "right", monitor: null } as const;
const LEFT = { side: "left", monitor: null } as const;
const win = (left: number, top: number, w = 1240, h = 780) => ({ left, top, right: left + w, bottom: top + h });

test("the light: a window touching the crossing edge is at full strength, from that side", () => {
  const h = heatFor(win(2560 - 1240, 300), DESK, RIGHT)!;
  assert.equal(h.side, "right");
  assert.equal(h.strength, 1);
});

test("the light fades as the window moves away from the edge, and is gone on the other monitor", () => {
  let last = 2;
  for (const x of [1320, 1000, 800, 500, 200, 0]) {
    const s = heatFor(win(x, 300), DESK, RIGHT)!.strength;
    assert.ok(s < last || (s === 0 && last === 0), `${x}: ${s} after ${last}`);
    assert.ok(s >= 0 && s <= 1);
    last = s;
  }
  assert.ok(heatFor(win(1100, 300), DESK, RIGHT)!.strength > 0.3, "a little way off it still glows");
  assert.equal(heatFor(win(-1700, 500), DESK, RIGHT)!.strength, 0, "on monitor 1, with no crossing edge");
});

test("the light: maximised or full screen on the edge's monitor is full strength", () => {
  // Windows' maximised rect reaches 8 px past the monitor on every side; full screen is the monitor.
  assert.equal(heatFor({ left: -8, top: -8, right: 2568, bottom: 1400 }, DESK, RIGHT)!.strength, 1);
  assert.equal(heatFor({ left: 0, top: 0, right: 2560, bottom: 1440 }, DESK, RIGHT)!.strength, 1);
  // Under monitor 2: a maximised window stops at the taskbar, 48 px above the edge.
  const under = heatFor({ left: -8, top: -8, right: 2568, bottom: 1392 + 8 - 48 }, DESK, { side: "bottom", monitor: 2 })!;
  assert.equal(under.side, "bottom");
  assert.equal(under.strength, 1);
});

test("the light: a window hanging over the edge is full strength", () => {
  assert.equal(heatFor(win(2000, 300), DESK, RIGHT)!.strength, 1);
});

test("the light: left mirrors right, on monitor 1's left edge", () => {
  const touching = heatFor(win(-1920, 500), DESK, LEFT)!;
  assert.equal(touching.side, "left");
  assert.equal(touching.strength, 1);
  const one = [{ number: 1, left: 0, top: 0, right: 1920, bottom: 1080 }];
  const l = heatFor(win(400, 100, 1000, 700), one, LEFT)!.strength;
  const r = heatFor(win(1920 - 400 - 1000, 100, 1000, 700), one, RIGHT)!.strength;
  assert.ok(l > 0 && l < 1, `${l}`);
  assert.ok(Math.abs(l - r) < 1e-9, `${l} vs ${r}`);
  assert.equal(heatFor(win(800, 300), DESK, LEFT)!.strength, 0, "on monitor 2, far from it");
});

test("the light: under a monitor, from that monitor's bottom edge", () => {
  const under1 = { side: "bottom", monitor: 1 } as const;
  const h = heatFor(win(-1600, 1438 - 780), DESK, under1)!;
  assert.equal(h.side, "bottom");
  assert.equal(h.strength, 1);
  assert.ok(heatFor(win(-1600, 400), DESK, under1)!.strength < 1);
  assert.ok(heatFor(win(800, 1440 - 780), DESK, under1)!.strength < heatFor(win(-1600, 1438 - 780), DESK, under1)!.strength, "on monitor 2: away from monitor 1's bottom");
});

test("the light: where along the window's side it comes in", () => {
  // Monitor 1 starts at y 358: a window above that on monitor 1 with the Mac on the left meets the
  // edge only along its lower part.
  const h = heatFor(win(-1920, 0, 1000, 800), DESK, LEFT)!;
  assert.ok(h.along > 0.6 && h.along <= 1, `${h.along}`);
  assert.ok(h.strength > 0.9);
  // Wholly above the edge (no overlap): beside it, not across from it, so none.
  const above = heatFor(win(-1920, -900, 1000, 800), DESK, LEFT)!;
  assert.equal(above.along, 1);
  assert.equal(above.strength, 0);
  // Overlapping all of it: the middle of the overlap.
  const mid = heatFor(win(1320, 330, 1240, 780), DESK, RIGHT)!;
  assert.ok(Math.abs(mid.along - 0.5) < 0.01, `${mid.along}`);
});

test("the light: no monitors, or a vanished one", () => {
  assert.equal(heatFor(win(0, 0), [], RIGHT), null);
  const gone = heatFor(win(-1600, 1438 - 780), DESK, { side: "bottom", monitor: 7 })!;
  assert.equal(gone.side, "bottom", "falls back to the leftmost monitor, as the desk does");
  assert.equal(gone.strength, 1);
});

test("the light: under a monitor, a window on the monitor beside it has none, even maximised (review I1)", () => {
  const under2 = { side: "bottom", monitor: 2 } as const;
  const under1 = { side: "bottom", monitor: 1 } as const;
  // Maximised on monitor 1: Windows' frame overhangs the shared boundary by 8 px.
  assert.equal(heatFor({ left: -1928, top: 350, right: 8, bottom: 1398 }, DESK, under2)!.strength, 0);
  // Maximised on monitor 2, the MacBook under monitor 1.
  assert.equal(heatFor({ left: -8, top: -8, right: 2568, bottom: 1400 }, DESK, under1)!.strength, 0);
  // A window on monitor 2 at its bottom-left corner, the MacBook under monitor 1.
  assert.equal(heatFor(win(0, 660), DESK, under1)!.strength, 0);
  // A window mostly on monitor 1, reaching over the boundary, gets it from monitor 1's bottom.
  const across = heatFor(win(-1000, 660), DESK, under1)!;
  assert.equal(across.strength, 1);
});

for (const scale of [1, 1.5, 2, 3]) {
  test(`the light at ${scale * 100}%: maximised beside the edge's monitor, none; on it, full (re-review I1')`, () => {
    const o = 8 * scale; // Windows' invisible frame: how far a maximised window overhangs its monitor
    const bar = 48 * scale; // the taskbar
    const max1 = { left: -1920 - o, top: 358 - o, right: 0 + o, bottom: 1438 - bar + o };
    const max2 = { left: -o, top: -o, right: 2560 + o, bottom: 1440 - bar + o };
    assert.equal(heatFor(max1, DESK, { side: "bottom", monitor: 2 })!.strength, 0, "monitor 1, MacBook under 2");
    assert.equal(heatFor(max2, DESK, { side: "bottom", monitor: 1 })!.strength, 0, "monitor 2, MacBook under 1");
    assert.equal(heatFor(max2, DESK, { side: "left", monitor: null })!.strength, 0, "monitor 2, MacBook left of 1");
    assert.equal(heatFor(max2, DESK, RIGHT)!.strength, 1, "monitor 2, MacBook right of it");
    assert.equal(heatFor(max2, DESK, { side: "bottom", monitor: 2 })!.strength, 1, "monitor 2, MacBook under it");
    assert.equal(heatFor(max1, DESK, LEFT)!.strength, 1, "monitor 1, MacBook left of it");
  });
}

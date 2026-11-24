import { useEffect, useRef, type RefObject } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Monitor, Placement } from "../api";
import { heatFor, type ScreenRect } from "./geometry";

/** The Mac's light follows where the window is (geometry.ts heatFor): sets `--ss-heat` (0..1) and
 *  `--ss-along` (a percentage along the side it comes in from) on the given elements. Updated on
 *  the window's move, resize and scale-change events and on a change of monitors or placement,
 *  never on a timer; outside React's renders, so a drag re-renders nothing (the CSS eases it).
 *  While `on` is false, or without the window's place (an error), the variables are unset and the
 *  light is as it was before it knew. */
export function useHeat(targets: RefObject<HTMLElement | null>[], monitors: Monitor[], placement: Placement, on: boolean) {
  const rect = useRef<ScreenRect | null>(null);
  const latest = useRef({ monitors, placement, on, targets });
  latest.current = { monitors, placement, on, targets };

  const apply = useRef(() => {
    const { monitors, placement, on, targets } = latest.current;
    const r = rect.current;
    if (!r) return;
    const heat = on ? heatFor(r, monitors, placement) : null;
    for (const t of targets) {
      const el = t.current;
      if (!el) continue;
      // Not connected (or not known): unset, so the light falls back to how it was before it knew
      // (connecting keeps its dim light) and the window-side light stays off.
      if (!on) el.style.removeProperty("--ss-heat");
      else el.style.setProperty("--ss-heat", heat ? heat.strength.toFixed(3) : "0");
      if (heat) el.style.setProperty("--ss-along", `${(heat.along * 100).toFixed(1)}%`);
    }
  }).current;

  useEffect(apply, [monitors, placement.side, placement.monitor, on, apply]);

  useEffect(() => {
    let alive = true;
    const unlisten: (() => void)[] = [];
    const win = getCurrentWindow();
    // A move that lands while a read is on its way is newer than that read: the read is dropped.
    let seq = 0;
    const read = () => {
      const mine = ++seq;
      return Promise.all([win.outerPosition(), win.outerSize()]).then(
        ([p, s]) => {
          if (!alive || mine !== seq) return;
          rect.current = { left: p.x, top: p.y, right: p.x + s.width, bottom: p.y + s.height };
          apply();
        },
        () => {},
      );
    };
    void read();
    const keep = (p: Promise<() => void>) =>
      void p.then(
        (un) => (alive ? unlisten.push(un) : un()),
        () => {},
      );
    keep(
      win.onMoved(({ payload }) => {
        seq++;
        const r = rect.current;
        if (!r) return void read();
        rect.current = { left: payload.x, top: payload.y, right: payload.x + r.right - r.left, bottom: payload.y + r.bottom - r.top };
        apply();
      }),
    );
    // A resize from the left or top edge moves the window too: read both.
    keep(win.onResized(() => void read()));
    keep(win.onScaleChanged(() => void read()));
    return () => {
      alive = false;
      unlisten.forEach((un) => un());
    };
  }, [apply]);
}

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { setPlacement, type Status } from "../api";
import { useMonitors } from "./useMonitors";
import { useHeat } from "./useHeat";
import { showDisplayOn } from "./startGuard";
import { Desktop16Regular } from "@fluentui/react-icons";
import { dragView, edgeFor, layoutDesk, nearestSlot, overlayMonitor, placementSlotId, slotFor, type DeskLayout, type DeskShape, type Rect, type Slot } from "./geometry";
import { ErrorLine, failureText, type Failure } from "./ErrorLine";

/** Placed by transform only: a move can glide without laying the page out again. */
const at = (x: number, y: number): CSSProperties => ({ transform: `translate(${x}px, ${y}px)` });
const box = (r: Rect): CSSProperties => ({ ...at(r.x, r.y), width: r.w, height: r.h });
/** The floor line is a 100px hairline stretched to its width, so a change of width glides too. */
const FLOOR_PX = 100;
/** pc.css knobs for trying other proportions (geometry.ts DeskShape); unset, the defaults hold. */
const KNOBS: [keyof DeskShape, string][] = [
  ["neck", "--ss-stand-neck"], ["macW", "--ss-mac-w"], ["macAspect", "--ss-mac-aspect"],
  ["neckW", "--ss-neck-w"], ["baseW", "--ss-base-w"], ["footH", "--ss-foot-h"],
];
function readKnobs(el: Element): Partial<DeskShape> {
  const style = getComputedStyle(el);
  const shape: Partial<DeskShape> = {};
  for (const [key, name] of KNOBS) {
    const v = parseFloat(style.getPropertyValue(name));
    if (Number.isFinite(v) && v >= 0) shape[key] = v;
  }
  return shape;
}

export function Desk({ status }: { status: Status }) {
  const monitors = useMonitors();
  const ref = useRef<HTMLDivElement>(null);
  const world = useRef<HTMLDivElement>(null);
  const macEl = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0, resizing: true });
  const [shape, setShape] = useState<Partial<DeskShape>>({}); // the drawing's proportions: geometry.ts's, or pc.css's knobs when set
  const pointer = useRef({ x: 0, y: 0 });
  const dragging = useRef(false);
  const [drag, setDrag] = useState<{ x: number; y: number } | null>(null);
  const grab = useRef({ dx: 0, dy: 0 });
  const frozen = useRef<DeskLayout | null>(null); // the layout as the drag began: the places hold still
  const [pending, setPending] = useState<string | null>(null); // the place just dropped on, until the status confirms it
  const [error, setError] = useState<Failure | null>(null);
  const auraEl = useRef<HTMLDivElement>(null);
  const heatEl = useRef<HTMLDivElement>(null);
  // How near the window is to the edge the cursor crosses to the Mac (useHeat sets --ss-heat).
  useHeat([auraEl, heatEl], monitors, status.placement, status.connection === "connected");

  // While the window is being resized everything follows at once; after that, moves glide.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    let settle: number | undefined;
    const measure = () => {
      setSize({ w: el.clientWidth, h: el.clientHeight, resizing: true });
      const knobs = readKnobs(el);
      setShape((old) => (JSON.stringify(old) === JSON.stringify(knobs) ? old : knobs));
      window.clearTimeout(settle);
      settle = window.setTimeout(() => setSize((s) => ({ ...s, resizing: false })), 200);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => {
      ro.disconnect();
      window.clearTimeout(settle);
    };
  }, []);

  // The arrangement is centred round the MacBook where it is going: the place just dropped on, or
  // the one the status has.
  const live = layoutDesk(monitors, size, pending ?? placementSlotId(status.placement, monitors), shape);
  const layout = drag && frozen.current ? frozen.current : live;
  // While dragging, the arrangement zooms out to show every place; the pointer maps back through it.
  const view = drag ? dragView(layout, size) : { s: 1, x: 0, y: 0 };
  const slot = slotFor(status.placement, layout);
  const target = pending ? layout.slots.find((s) => s.id === pending) ?? null : null;
  const here = target ?? slot;
  const pos = drag ?? (here ? { x: here.x, y: here.y } : null);
  const shown = status.state === "connected" ? overlayMonitor(monitors, status.viewer_monitor) : null;
  const windowed = status.settings.window_mode === "windowed";
  const edge = slot && !pending && !drag && status.mouse_sharing && status.connection === "connected" ? edgeFor(slot, layout) : null;
  const floorW = layout.floor.w;
  // One wallpaper across every monitor's screen, as one desktop picture spanning the displays:
  // each screen shows its own part of it.
  const span = layout.monitors.length
    ? {
        x: Math.min(...layout.monitors.map((m) => m.x)),
        y: Math.min(...layout.monitors.map((m) => m.y)),
        w: Math.max(...layout.monitors.map((m) => m.x + m.w)) - Math.min(...layout.monitors.map((m) => m.x)),
        h: Math.max(...layout.monitors.map((m) => m.y + m.h)) - Math.min(...layout.monitors.map((m) => m.y)),
      }
    : { x: 0, y: 0, w: 1, h: 1 };
  // The Mac's light: it comes into the window through the edge the cursor crosses, from the
  // MacBook's side, while the Mac is connected (dimmer while connecting). It follows the MacBook
  // where it is going, and the zoom; while the MacBook is in the air it waits, dimmed, at its place.
  const seam = here ? edgeFor(here, layout) : null;
  const light = status.connection === "connected" ? "on" : status.connection === "connecting" ? "dim" : "off";
  const heatSide = status.placement.side === "left" ? "left" : status.placement.side === "bottom" ? "bottom" : "right";
  const lightAt = seam
    ? at(view.x + view.s * (seam.x + seam.w / 2), view.y + view.s * (seam.y + seam.h / 2))
    : undefined;

  useEffect(() => () => void (dragging.current = false), []); // no frames after unmount

  // The status caught up with the drop (or a dead call never will: let go after a while).
  useEffect(() => {
    if (pending && slot?.id === pending) setPending(null);
  }, [pending, slot?.id]);
  useEffect(() => {
    if (!pending) return;
    const t = window.setTimeout(() => setPending(null), 4000);
    return () => window.clearTimeout(t);
  }, [pending]);

  // The MacBook stays under the pointer while the arrangement zooms out: the pointer is mapped
  // through the zoom as it is drawn this frame (mid-glide too), every frame of the drag.
  const follow = () => {
    const el = world.current;
    const stage = ref.current;
    if (!el || !stage) return;
    const r = stage.getBoundingClientRect();
    const t = getComputedStyle(el).transform;
    const m = t && t !== "none" ? new DOMMatrixReadOnly(t) : new DOMMatrixReadOnly();
    const s = m.a || 1;
    const p = { x: (pointer.current.x - r.left - m.e) / s - grab.current.dx, y: (pointer.current.y - r.top - m.f) / s - grab.current.dy };
    // Drawn this frame (React's render of it can land a frame later, behind the glide).
    if (macEl.current) macEl.current.style.transform = at(p.x, p.y).transform as string;
    setDrag(p);
  };
  const down = (e: React.PointerEvent) => {
    if (!here || e.button !== 0 || drag) return;
    const r = ref.current!.getBoundingClientRect();
    grab.current = { dx: e.clientX - r.left - here.x, dy: e.clientY - r.top - here.y }; // not zoomed yet
    pointer.current = { x: e.clientX, y: e.clientY };
    e.currentTarget.setPointerCapture(e.pointerId);
    frozen.current = live;
    setPending(null);
    setDrag({ x: here.x, y: here.y });
    dragging.current = true;
    const frame = () => {
      if (!dragging.current) return;
      follow();
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  };
  const move = (e: React.PointerEvent) => {
    if (!drag) return;
    pointer.current = { x: e.clientX, y: e.clientY };
  };
  const up = (e: React.PointerEvent) => {
    dragging.current = false;
    if (!drag) return;
    const best = e.type === "pointerup" ? nearestSlot(layout, drag.x, drag.y) : null;
    frozen.current = null;
    setDrag(null);
    if (!best || best.id === slot?.id) return;
    place(best);
  };
  // A monitor's Show here: the Mac's display there (started, or moved if it is running).
  const canShow = status.connection === "connected" && !drag;
  const showOn = (n: number) => {
    setError(null);
    showDisplayOn(status, n).catch((err) => setError({ text: failureText(err), retry: () => showOn(n) }));
  };
  const place = (to: Slot) => {
    setPending(to.id);
    setPlacement(to.side, to.monitor).then(
      () => setError(null),
      (err) => {
        setPending(null); // snap back
        setError({ text: failureText(err), retry: () => place(to) });
      },
    );
  };

  return (
    <>
    {/* Outside the stage, whose clip would cut it to a box: the light reaches across the window.
        The light in through the window's own side covers the window. */}
    <div ref={heatEl} className={`ss-heat ${light} ${heatSide}`} aria-hidden><div className="ss-heat-light" /></div>
    <div ref={auraEl} className={`ss-aura ${light} ${here?.side === "bottom" ? "under" : here?.side ?? ""}${size.resizing ? " still" : ""}${drag ? " lifted" : ""}${seam ? "" : " gone"}`} style={lightAt} aria-hidden>
      <div className="ss-aura-light" />
    </div>
    <div className={`ss-stage${size.resizing ? " ss-still" : ""}`} ref={ref}>
      <div className="ss-world" ref={world} style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.s})` }}>
      {layout.monitors.length > 0 && (
        <div className="ss-floor" style={{ transform: `translate(${layout.floor.x}px, ${layout.deskY}px) scaleX(${floorW / FLOOR_PX})` }} />
      )}
      {layout.slots.map((s) => (
        <div key={s.id} className={`ss-slot${drag ? " visible" : ""}`} style={{ ...at(s.x, s.y), width: layout.mac.w, height: layout.mac.h }} />
      ))}
      {layout.monitors.map((m) => (
        <div key={m.number} className={`ss-desk-mon${canShow && shown !== m.number ? " can-show" : ""}`}>
          <div className="ss-neck" style={box(m.neck)} />
          <div className="ss-foot" style={box(m.base)} />
          <div className="ss-bezel" style={box(m)}>
            <div className="ss-scr">
              <div className="ss-wall">
                <div className="ss-wall-pic" style={{ left: span.x - m.x, top: span.y - m.y, width: span.w, height: span.h }} />
              </div>
              <div className={`ss-macscreen${shown === m.number ? " on" : ""}${windowed ? " windowed" : ""}`}>
                <div className="w1" />
                <div className="w2" />
              </div>
            </div>
            <span className={`ss-num${shown === m.number ? " sel" : ""}`}>{m.number}</span>
            {canShow && shown !== m.number && (
              <button type="button" className="ss-showhere" aria-label={`Show here, monitor ${m.number}`} onClick={() => showOn(m.number)}>
                <Desktop16Regular />Show here
              </button>
            )}
          </div>
        </div>
      ))}
      <div className={`ss-edge${edge ? ` on ${slot?.side ?? ""}` : ""}${edge && edge.w > edge.h ? " across" : ""}`} style={edge ? box(edge) : undefined} />
      {pos && (
        <div
          ref={macEl}
          className={`ss-mac${drag ? " dragging" : ""}`}
          style={{ ...at(pos.x, pos.y), width: layout.mac.w, height: layout.mac.h }}
          onPointerDown={down}
          onPointerMove={move}
          onPointerUp={up}
          onPointerCancel={up}
          onLostPointerCapture={up}
        >
          <div className="ss-mac-body">
            <div className="ss-lid"><div className="ss-lid-scr"><i /></div></div>
            <div className="ss-deck" />
            <div className="ss-lip"><i /></div>
          </div>
        </div>
      )}
      </div>
      {error && <ErrorLine className="ss-desk-err" failure={error} />}
    </div>
    </>
  );
}

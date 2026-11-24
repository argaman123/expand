import { useEffect, useId, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Dropdown, Menu, MenuItemRadio, MenuList, MenuPopover, MenuTrigger, Option, Popover, PopoverSurface, PopoverTrigger,
  Toolbar, ToolbarButton, ToolbarDivider, Tooltip,
} from "@fluentui/react-components";
import {
  ChevronDown16Regular, Desktop20Regular, Dismiss20Regular, FullScreenMinimize20Regular, Settings20Regular,
} from "@fluentui/react-icons";
import {
  barMenuOpen, barShape, displayFullScreen, displayPullDown, displayStop, displayWindow, getMonitors, saveSettings,
  type BarShape, type DisplayMode, type Monitor, type SettingsInput, type Status,
} from "../api";
import { bitrateOptions, fpsLabel, fpsOptions, rateLabel, resKey, resLabel, resolutionOptions } from "../pc/displayOptions";
import { PEEK_MS, barTarget } from "./peek";

/** How long the bar stays after the pointer leaves (ms). */
const LEAVE_MS = 350;
/** The bar's fade in and out (ms), as in bar.css. */
const FADE_MS = 180;
/** How long the bar takes to slide away (bar.css): the region stays till then, so it never snaps (round 4, the owner). */
const HIDE_MS = 480;
/** The keyboard stays this long after the last menu closes (ms): switching from the menu to the
 *  popover closes one just before the other opens. */
const RELEASE_MS = 150;
/** How much each shape shows: the window's region grows before the page draws more, and shrinks
 *  once it drew less. */
const RANK: Record<BarShape, number> = { hidden: 0, bar: 1, menu: 2 };
/** The display settings stay open through the restart a change causes, at most this long (ms). */
const PIN_MS = 20000;
/** Once the restarted display is in, the popover takes the keyboard back after this (ms): the
 *  display window takes the front from the viewer's windows first. */
const RETAKE_MS = 400;
/** How long the bar's page may take to hear it has the keyboard after it was put in front (ms). */
const FOCUS_MS = 200;
/** Pulling the bar down this far (CSS px, straight down) makes the display a window. */
const PULL_PX = 30;

type Lock = "menu" | "pop";

const quietly = (p: Promise<unknown>) => void p.catch(() => {});
const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Follows a Tauri event while mounted. */
function useEvent<T>(name: string, handle: (payload: T) => void) {
  const latest = useRef(handle);
  latest.current = handle;
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void listen<T>(name, (e) => alive && latest.current(e.payload)).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [name]);
}

const monitorLabel = (m: Monitor) => `${m.number} · ${m.right - m.left} × ${m.bottom - m.top}`;

/** The bar over the full-screen display: hidden until the pointer comes near the top centre (the
 *  app tells), then the whole bar (round 3: no tab step), until the pointer goes. The window
 *  never moves or resizes; only its region (what shows and takes
 *  the pointer, bar.rs) changes: it grows before the page draws more, and shrinks after. */
export function NotchBar({ status }: { status: Status | null }) {
  const ids = { res: useId(), fps: useId(), rate: useId() };
  const notch = useRef<HTMLDivElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [popOpen, setPopOpen] = useState(false);
  const [near, setNear] = useState(false); // the pointer is near the top centre (bar.rs)
  const [drawn, setDrawn] = useState<BarShape>("hidden"); // what is drawn (inside the region)
  const applied = useRef<BarShape>("hidden"); // the region asked for last
  // Popup mode: the window holds the display window's title bar settings popover (bar.rs), at its
  // top right corner, and nothing else.
  const [popup, setPopup] = useState(false);
  const [anchor, setAnchor] = useState<HTMLDivElement | null>(null);
  const [mode, setMode] = useState<DisplayMode | null>(null);
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  const leaveTimer = useRef<number | undefined>(undefined);
  const menus = useRef(new Set<Lock>()); // open, or opening
  const open = hover || menuOpen || popOpen;
  const hoverRef = useRef(hover);
  hoverRef.current = hover;
  // A tooltip shows: the region has room round the bar only then (bar.rs), so nothing see-through
  // takes the clicks meant for the Mac's menu bar under it.
  const [tip, setTip] = useState(false);
  const tipRef = useRef(tip);
  tipRef.current = tip;
  const tips = useRef(new Set<string>());
  const onTip = (which: string) => (_: unknown, d: { visible: boolean }) => {
    if (d.visible) tips.current.add(which);
    else tips.current.delete(which);
    setTip(tips.current.size > 0);
  };

  const enter = () => {
    window.clearTimeout(leaveTimer.current);
    leaveTimer.current = undefined;
    setHover(true);
  };
  const leave = () => {
    window.clearTimeout(leaveTimer.current);
    leaveTimer.current = window.setTimeout(() => {
      leaveTimer.current = undefined;
      setHover(false);
    }, LEAVE_MS);
  };

  // A menu or popover has the keyboard while it is open, and a little after the last one closes,
  // so switching from one to the other (Fluent closes the first just before the second opens)
  // doesn't move the keyboard.
  const releaseTimer = useRef<number | undefined>(undefined);
  const release = (now: boolean) => {
    window.clearTimeout(releaseTimer.current);
    releaseTimer.current = undefined;
    if (now) quietly(barMenuOpen(false));
    else
      releaseTimer.current = window.setTimeout(() => {
        releaseTimer.current = undefined;
        if (menus.current.size === 0) quietly(barMenuOpen(false));
      }, RELEASE_MS);
  };
  const setLock = (which: Lock, on: boolean) => (which === "menu" ? setMenuOpen(on) : setPopOpen(on));
  const ask = (which: Lock, want: boolean) => {
    if (want) {
      window.clearTimeout(releaseTimer.current);
      releaseTimer.current = undefined;
      menus.current.add(which);
      if (which === "menu") quietly(getMonitors().then(setMonitors));
      quietly(barMenuOpen(true));
      setLock(which, true);
      return;
    }
    const had = menus.current.delete(which);
    setLock(which, false);
    if (had && menus.current.size === 0) release(false);
  };
  /** Closes every menu; `now`: give up the room and the keyboard at once (the bar is going). */
  /** Forgets any tooltip: one showing as the bar goes may never say it went. */
  const noTips = () => {
    tips.current.clear();
    setTip(false);
  };
  // A display setting changed: the stream restarts with it, and the viewer's new windows take the
  // front (the popover would close as the bar loses it). The popover stays open through that,
  // and takes the keyboard back once the new display is in, so several values can be changed in
  // a row.
  const pinned = useRef(false);
  const pinTimer = useRef<number | undefined>(undefined);
  const pin = (on: boolean) => {
    window.clearTimeout(pinTimer.current);
    pinned.current = on;
    pinTimer.current = on ? window.setTimeout(() => (pinned.current = false), PIN_MS) : undefined;
  };
  const closeMenus = (now: boolean) => {
    const had = menus.current.size > 0 || releaseTimer.current !== undefined;
    pin(false);
    menus.current.clear();
    setMenuOpen(false);
    setPopOpen(false);
    if (had) release(now);
  };
  /** Closes the bar's menus and hover at once (after an action that moves, ends or windows the
   *  display); it stays only while the pointer is near. */
  const collapse = () => {
    window.clearTimeout(leaveTimer.current);
    leaveTimer.current = undefined;
    setHover(false);
    closeMenus(true);
    noTips();
  };

  // Pulling down: a press on the bar (not in a menu or popover, which React also
  // reports here), then the pointer this far below where it went down, makes the display a window
  // that follows the pointer. The app takes the mouse from here on, so the page never hears the
  // button go up; it lets go of the pull at once. A press that doesn't travel is a click as before.
  const endPull = useRef<(() => void) | undefined>(undefined);
  const pointerDown = (e: React.PointerEvent) => {
    endPull.current?.();
    if (e.pointerType !== "mouse" || e.button !== 0 || !notch.current?.contains(e.target as Node)) return;
    const { pointerId, screenX, screenY } = e;
    const move = (m: PointerEvent) => {
      if (m.pointerId !== pointerId) return;
      // The button went up without the page hearing of it: no pull.
      if ((m.buttons & 1) === 0) return end();
      const dy = m.screenY - screenY;
      if (dy < PULL_PX || dy <= Math.abs(m.screenX - screenX)) return;
      end();
      collapse();
      quietly(displayPullDown());
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      endPull.current = undefined;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    endPull.current = end;
  };
  useEffect(() => () => endPull.current?.(), []);

  // The open bar's own size (bar.rs adds room while a tooltip shows).
  const openSize = (): [number, number] => {
    const b = bar.current;
    if (!b) return [560, 44];
    // Even, so the bar sits on whole pixels in the middle of the window.
    return [2 * Math.ceil(b.offsetWidth / 2), Math.ceil(b.offsetHeight)];
  };
  const askShape = (shape: BarShape) => {
    applied.current = shape;
    const [w, h] = openSize();
    return barShape(shape, w, h, tipRef.current).catch(() => {});
  };

  // The bar shows by itself for a moment (a peek) when it comes up over the full-screen display
  // (it went full screen, or came back in front: the app says `bar-shown`), so it is clear where
  // it is; then it goes, unless the pointer is near. One timer per peek; nothing runs while
  // nothing changes.
  const [peeking, setPeeking] = useState(false);
  const peekTimer = useRef<number | undefined>(undefined);
  const peek = (on: boolean) => {
    window.clearTimeout(peekTimer.current);
    peekTimer.current = on ? window.setTimeout(() => setPeeking(false), PEEK_MS) : undefined;
    setPeeking(on);
  };
  useEffect(() => () => window.clearTimeout(peekTimer.current), []);

  // What shows: the bar while open, near or peeking (all of the window while a menu or popover
  // is open under it), else nothing. More: the region grows, then it is drawn (the bar fades in).
  // Less: drawn at once (the fade out), and the region shrinks once that is done.
  const target: BarShape = barTarget({ popup, popOpen, open, menu: menuOpen || popOpen, near, peeking });
  const step = useRef(0);
  useEffect(() => {
    const n = ++step.current;
    const was = applied.current;
    if (RANK[target] >= RANK[was]) {
      void askShape(target).then(() => step.current === n && setDrawn(target));
      return;
    }
    setDrawn(target);
    if (target !== "bar") noTips();
    // Going: the region stays while the bar slides away, so it never snaps out of sight (round 4,
    // the owner). The title bar's popover has nothing to animate away.
    const wait = reducedMotion() || popup ? 0 : target === "hidden" ? HIDE_MS : FADE_MS;
    const t = window.setTimeout(() => {
      void askShape(target);
      if (target === "hidden") setPopup(false);
    }, wait);
    return () => window.clearTimeout(t);
  }, [target]);

  // The open bar's region follows its size (the Mac's name and the display data) and a tooltip
  // showing or going.
  const resize = () => {
    // Not while a menu or popover is open or opening: a late "bar" would end it in the app and
    // hand the keyboard back mid-menu.
    if (applied.current === "bar" && menus.current.size === 0) void askShape("bar");
  };
  useEffect(resize, [tip]);
  useEffect(() => {
    const b = bar.current;
    if (!b) return;
    const watch = new ResizeObserver(resize);
    watch.observe(b);
    return () => watch.disconnect();
  }, []);

  useEffect(() => {
    let alive = true;
    void getMonitors().then((m) => alive && setMonitors(m), () => {});
    // In front only while a menu is open: a click elsewhere closes it. A blur that comes and goes
    // while the bar is put in front (as a menu opens) closes nothing.
    let later: number | undefined;
    const blur = () => {
      window.clearTimeout(later);
      later = window.setTimeout(() => {
        if (alive && !document.hasFocus() && !pinned.current) closeMenus(true);
      }, 100);
    };
    window.addEventListener("blur", blur);
    return () => {
      alive = false;
      window.clearTimeout(later);
      window.clearTimeout(releaseTimer.current);
      window.clearTimeout(pinTimer.current);
      window.removeEventListener("blur", blur);
    };
  }, []);
  useEvent<DisplayMode>("display-mode", (m) => {
    setMode(m);
    quietly(getMonitors().then(setMonitors));
  });
  // The app hid the bar (the display is no longer full screen, or not in front): it comes back
  // as nothing.
  useEvent<null>("bar-closed", () => {
    endPull.current?.();
    peek(false);
    collapse();
    setPopup(false);
    setNear(false);
    setDrawn("hidden");
    applied.current = "hidden";
  });
  // The restarted display is in: the popover kept open takes the keyboard back. Should Windows
  // refuse the bar the front, no blur would ever close the popover (and its region would keep
  // covering the top of the display): it closes then.
  useEvent<boolean>("display-viewer", (inIt) => {
    if (!inIt || !pinned.current) return;
    window.setTimeout(() => {
      if (!pinned.current) return;
      pin(false);
      if (menus.current.size === 0) return;
      void barMenuOpen(true)
        .catch(() => {})
        .then(() => window.setTimeout(() => !document.hasFocus() && closeMenus(true), FOCUS_MS));
    }, RETAKE_MS);
  });
  // The display window's title bar settings button: the popover opens here (the window is
  // already under the button, all of it showing).
  useEvent<null>("bar-settings", () => {
    endPull.current?.();
    peek(false);
    collapse();
    setNear(false);
    applied.current = "menu";
    setPopup(true);
    ask("pop", true);
  });
  // The pointer came near the top centre, or went.
  useEvent<boolean>("bar-near", (n) => setNear(n));
  // The bar came up over the full-screen display: it shows by itself a moment.
  useEvent<null>("bar-shown", () => peek(true));
  // The pointer was seen leaving the open bar without the page hearing of it.
  useEvent<null>("bar-pointer-out", () => {
    if (hoverRef.current && leaveTimer.current === undefined) leave();
  });

  const s = status?.settings;
  const name = status?.peer_name ?? "Mac";
  const current = (mode && mode.monitor > 0 ? mode.monitor : null) ?? status?.viewer_monitor ?? s?.monitor ?? null;

  // A display setting restarts the stream with it; the mode and the monitor stay as they are.
  const save = (patch: Partial<SettingsInput>) => {
    if (!s) return;
    pin(true);
    saveSettings({
      width: s.width, height: s.height, fps: s.fps, bitrate_mbps: s.bitrate_mbps, window_mode: s.window_mode,
      monitor: s.monitor, ...patch,
    }).catch(() => pin(false));
  };
  const pickMonitor = (n: number) => {
    collapse();
    if (n && n !== current) quietly(displayFullScreen(n));
  };

  const cur = s ? { w: s.width, h: s.height } : null;
  const resOpts = cur ? resolutionOptions(cur) : [];

  // The display settings: in the bar's popover, or in the title bar's (popup mode).
  const panel = (
    <PopoverSurface className="ss-npanel">
      {s && cur && (
        <div className="ss-npb">
          <div className="ss-nrow">
            <label htmlFor={ids.res}>Resolution</label>
            <Dropdown
              id={ids.res} inlinePopup size="small" className="ss-ndd"
              value={resLabel(cur)} selectedOptions={[resKey(cur)]}
              onOptionSelect={(_, d) => {
                const r = resOpts.find((x) => resKey(x) === d.optionValue);
                if (r && (r.w !== s.width || r.h !== s.height)) save({ width: r.w, height: r.h });
              }}
            >
              {resOpts.map((r) => <Option key={resKey(r)} value={resKey(r)} text={resLabel(r)}>{resLabel(r)}</Option>)}
            </Dropdown>
          </div>
          <div className="ss-nrow">
            <label htmlFor={ids.fps}>Frame rate</label>
            <Dropdown
              id={ids.fps} inlinePopup size="small" className="ss-ndd"
              value={fpsLabel(s.fps)} selectedOptions={[String(s.fps)]}
              onOptionSelect={(_, d) => {
                const v = Number(d.optionValue);
                if (v && v !== s.fps) save({ fps: v });
              }}
            >
              {fpsOptions(s.fps).map((v) => <Option key={v} value={String(v)} text={fpsLabel(v)}>{fpsLabel(v)}</Option>)}
            </Dropdown>
          </div>
          <div className="ss-nrow">
            <label htmlFor={ids.rate}>Bitrate</label>
            <Dropdown
              id={ids.rate} inlinePopup size="small" className="ss-ndd"
              value={rateLabel(s.bitrate_mbps)} selectedOptions={[String(s.bitrate_mbps)]}
              onOptionSelect={(_, d) => {
                const v = Number(d.optionValue);
                if (v && v !== s.bitrate_mbps) save({ bitrate_mbps: v });
              }}
            >
              {bitrateOptions(s.bitrate_mbps).map((v) => <Option key={v} value={String(v)} text={rateLabel(v)}>{rateLabel(v)}</Option>)}
            </Dropdown>
          </div>
        </div>
      )}
    </PopoverSurface>
  );

  return (
    <>
      <div
        ref={notch}
        className={`ss-notch${drawn !== "hidden" ? " ss-near" : ""}${RANK[drawn] >= RANK.bar ? " ss-open" : ""}`}
        style={popup ? { display: "none" } : undefined}
        onMouseEnter={enter}
        onMouseLeave={leave}
        onPointerDown={pointerDown}
      >
        <div className="ss-nbar">
          <Toolbar ref={bar} className="ss-tbar" size="medium" aria-label={name}>
            <span className="ss-who"><span className="ss-dot" />{name}</span>
            <Popover open={popOpen && !popup} onOpenChange={(_, d) => ask("pop", d.open)} positioning="below" trapFocus>
              <PopoverTrigger disableButtonEnhancement>
                <Tooltip content="Display settings" relationship="description" positioning="below" withArrow onVisibleChange={onTip("settings")}>
                  <ToolbarButton className="ss-data" icon={<Settings20Regular />} aria-label="Display settings">
                    <span className="ss-data-text">{s ? `${resLabel(cur!)} · ${fpsLabel(s.fps)}` : ""}</span>
                  </ToolbarButton>
                </Tooltip>
              </PopoverTrigger>
              {panel}
            </Popover>
            <ToolbarDivider />
            <Menu
              open={menuOpen}
              onOpenChange={(_, d) => ask("menu", d.open)}
              checkedValues={{ mon: current ? [String(current)] : [] }}
              onCheckedValueChange={(_, d) => pickMonitor(Number(d.checkedItems[0]))}
              positioning="below"
            >
              <MenuTrigger disableButtonEnhancement>
                <Tooltip content="Monitor" relationship="label" positioning="below" withArrow onVisibleChange={onTip("monitor")}>
                  <ToolbarButton icon={<Desktop20Regular />} className="ss-mon">
                    {current ?? ""}
                    <ChevronDown16Regular className="ss-chev" />
                  </ToolbarButton>
                </Tooltip>
              </MenuTrigger>
              <MenuPopover>
                <MenuList>
                  {monitors.map((m) => (
                    <MenuItemRadio key={m.number} name="mon" value={String(m.number)}>{monitorLabel(m)}</MenuItemRadio>
                  ))}
                </MenuList>
              </MenuPopover>
            </Menu>
            <Tooltip content="Window" relationship="label" positioning="below" withArrow onVisibleChange={onTip("window")}>
              <ToolbarButton
                icon={<FullScreenMinimize20Regular />}
                onClick={() => {
                  collapse();
                  quietly(displayWindow());
                }}
              />
            </Tooltip>
            <Tooltip content="Close display" relationship="label" positioning="below" withArrow onVisibleChange={onTip("close")}>
              <ToolbarButton
                icon={<Dismiss20Regular />}
                onClick={() => {
                  collapse();
                  quietly(displayStop());
                }}
              />
            </Tooltip>
          </Toolbar>
        </div>
      </div>
      {popup && (
        <div className="ss-popup">
          <div ref={setAnchor} className="ss-panchor" />
          {anchor && (
            <Popover
              open={popOpen}
              onOpenChange={(_, d) => ask("pop", d.open)}
              positioning={{ target: anchor, position: "below", align: "end" }}
              trapFocus
            >
              {panel}
            </Popover>
          )}
        </div>
      )}
    </>
  );
}

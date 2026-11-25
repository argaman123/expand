// A stand-in for the Tauri runtime, so the pages can be looked at in a plain browser while
// developing (`npm run dev`, then http://localhost:1420/?mock=connected). Loaded by the pages only
// in dev and only when the real runtime is absent: it is never in a production bundle.
//
// `?mock=` picks the state: connected (default), display (the display running), off (not
// connected), unknown (not paired), connecting, failed. `&side=left|right|bottom&under=1|2` places
// the MacBook; `&latency=N` sets the round trip; `&old=1` is a backend without the newer
// commands (no start_display_on_connect / ctrl_as_cmd). `window.__mock.set({...})` changes the status
// live, as a `state-changed` event would.
// The window's place on the desktop (physical px, as Tauri gives it): `&winx=N&winy=N` (default:
// the middle of monitor 2), `&max=N` maximised on monitor N, `&scale=1.5`. `window.__mock.moveWindow(x, y)`
// moves it, as a drag would (a `tauri://move` event).
import type { Monitor, Status } from "../api";
import "./lookOptions"; // the round 3 options (?wall=c)

const MONITORS: Monitor[] = [
  { number: 1, left: -1920, top: 358, right: 0, bottom: 1438 },
  { number: 2, left: 0, top: 0, right: 2560, bottom: 1440 },
];

const q = new URLSearchParams(location.search);
const scenario = q.get("mock") ?? "connected";

const peers = [
  { key: "air", name: "MacBook Air", host: "192.168.1.20", addresses: ["192.168.1.20", "100.64.0.20"], current: true, allowed: true, paired: true, connected: false },
  { key: "mini", name: "Mac mini", host: "192.168.1.31", addresses: ["192.168.1.31"], current: false, allowed: true, paired: true, connected: false },
];

function base(): Status {
  const side = (q.get("side") ?? "right") as Status["placement"]["side"];
  const under = q.get("under");
  return {
    role: "client",
    state: "disconnected",
    part: null,
    reason: null,
    peer_name: "MacBook Air",
    peer_host: "192.168.1.20",
    paired: true,
    settings: { width: 2560, height: 1440, fps: 60, bitrate_mbps: 40, window_mode: "borderless", monitor: 2 },
    config_error: null,
    mouse_sharing: true,
    mac_side: side,
    mouse_state: "running",
    mouse_reason: null,
    connection: "connected",
    busy_by: null,
    peers: peers.map((p) => ({ ...p, connected: p.current })),
    auto_connect: true,
    placement: { side, monitor: under ? Number(under) : side === "bottom" ? 1 : null },
    uses: null,
    pairing: null,
    close_to_tray: true,
    clipboard_sharing: true,
    language_follow: false,
    peer_addresses: ["192.168.1.20"],
    latency_ms: q.has("latency") ? Number(q.get("latency")) : 3,
    viewer_monitor: 2,
    ...(q.has("old") ? {} : { start_display_on_connect: true, ctrl_as_cmd: true, reverse_scroll: true, mac_sound: true }),
  };
}

function initial(): Status {
  const s = base();
  switch (scenario) {
    case "display":
      return { ...s, state: "connected" };
    case "windowed":
      return { ...s, state: "connected", settings: { ...s.settings, window_mode: "windowed" } };
    case "off":
      return { ...s, connection: "off", mouse_sharing: false, mouse_state: "off", latency_ms: null, peers: peers };
    case "unknown":
      return { ...s, connection: "unknown", paired: false, mouse_sharing: false, latency_ms: null, peers: [{ ...peers[0], paired: false }] };
    case "connecting":
      return { ...s, connection: "connecting", latency_ms: null, state: "connecting" };
    case "failed":
      return { ...s, state: "failed", part: "Moonlight", reason: "the stream ended unexpectedly" };
    case "long":
      return {
        ...s,
        peer_name: "Studio MacBook Pro 16-inch (Work)",
        peer_host: "fe80::1c2b:3d4e:5f60:7182",
        peers: [{ ...peers[0], name: "Studio MacBook Pro 16-inch (Work)", host: "fe80::1c2b:3d4e:5f60:7182", connected: true }],
      };
    default:
      return s;
  }
}

let status = initial();

const scale = Number(q.get("scale") ?? 1) || 1;
const maxOn = MONITORS.find((m) => m.number === Number(q.get("max")));
// Windows' maximised rect reaches 8 px past the monitor, and stops at a 48 px taskbar.
const win = maxOn
  ? { x: maxOn.left - 8, y: maxOn.top - 8, w: maxOn.right - maxOn.left + 16, h: maxOn.bottom - maxOn.top - 48 + 16, max: true }
  : {
      x: q.has("winx") ? Number(q.get("winx")) : Math.round((2560 - innerWidth * scale) / 2),
      y: q.has("winy") ? Number(q.get("winy")) : Math.round((1440 - innerHeight * scale) / 2),
      w: Math.round(innerWidth * scale),
      h: Math.round(innerHeight * scale),
      max: false,
    };
let nextId = 1;
const callbacks = new Map<number, (payload: unknown) => void>();
const listeners = new Map<string, Set<number>>();

function emit(event: string, payload: unknown) {
  for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload });
}

const replies: Record<string, (args: Record<string, unknown>) => unknown> = {
  get_status: () => status,
  get_monitors: () => MONITORS,
  backdrop: () => "solid",
  get_system: () => ({ missing: q.has("missing") ? "Moonlight" : null, start_at_login: true, screen_recording: null, accessibility: null }),
  check_update: () => ({ current: "1.1.0", latest: q.has("update") ? "1.2.0" : null, url: "", in_place: true }),
  // The real one ends the app; here it just keeps going.
  install_update: () => new Promise(() => {}),
  find_macs: () => new Promise((r) => setTimeout(() => r([{ name: "MacBook Air", host: "192.168.1.20", id: "air", pairing: true }]), 600)),
  set_placement: ({ side, monitor }) => {
    set({ placement: { side: side as Status["placement"]["side"], monitor: (monitor as number | null) ?? null } });
  },
  set_mouse_sharing: ({ enabled }) => set({ mouse_sharing: !!enabled }),
  set_clipboard_sharing: ({ enabled }) => set({ clipboard_sharing: !!enabled }),
  set_language_follow: ({ enabled }) => set({ language_follow: !!enabled }),
  set_auto_connect: ({ enabled }) => set({ auto_connect: !!enabled }),
  set_close_to_tray: ({ enabled }) => set({ close_to_tray: !!enabled }),
  set_start_display_on_connect: ({ enabled }) => set({ start_display_on_connect: !!enabled }),
  set_ctrl_as_cmd: ({ enabled }) => set({ ctrl_as_cmd: !!enabled }),
  set_reverse_scroll: ({ enabled }) => set({ reverse_scroll: !!enabled }),
  set_mac_sound: ({ enabled }) => set({ mac_sound: !!enabled }),
  display_start: () => {
    set({ state: "connecting" });
    setTimeout(() => set({ state: "connected" }), 1200);
  },
  display_stop: () => set({ state: "disconnected" }),
  display_window: ({ monitor }) =>
    set({ settings: { ...status.settings, window_mode: "windowed" }, ...(monitor != null ? { viewer_monitor: monitor as number } : {}) }),
  display_full_screen: ({ monitor }) =>
    set({ settings: { ...status.settings, window_mode: "borderless" }, ...(monitor != null ? { viewer_monitor: monitor as number } : {}) }),
  save_settings: ({ settings }) => {
    const s = settings as Status["settings"] & { monitor_picked?: boolean };
    const { monitor_picked: _picked, ...saved } = s;
    set({ settings: { ...status.settings, ...saved }, ...(s.monitor != null && status.state !== "connected" ? { viewer_monitor: s.monitor } : {}) });
  },
  disconnect_mac: () => set({ connection: "off", latency_ms: null }),
  connect_mac: () => {
    set({ connection: "connecting" });
    setTimeout(() => set({ connection: "connected", latency_ms: 3 }), 900);
  },
  reconnect_mac: () => replies.connect_mac({}),
  "plugin:app|version": () => "1.1.0",
  "plugin:window|outer_position": () => ({ x: win.x, y: win.y }),
  "plugin:window|outer_size": () => ({ width: win.w, height: win.h }),
  "plugin:window|scale_factor": () => scale,
  "plugin:window|is_maximized": () => win.max,
  "plugin:window|is_fullscreen": () => false,
  "plugin:event|listen": ({ event, handler }) => {
    const set = listeners.get(event as string) ?? new Set<number>();
    set.add(handler as number);
    listeners.set(event as string, set);
    return handler;
  },
  "plugin:event|unlisten": () => undefined,
};

function set(patch: Partial<Status>) {
  status = { ...status, ...patch };
  emit("state-changed", status);
}

declare global {
  interface Window {
    __mock: {
      set: (patch: Partial<Status>) => void;
      emit: (event: string, payload: unknown) => void;
      status: () => Status;
      calls: string[];
      moveWindow: (x: number, y: number) => void;
    };
  }
}
const calls: string[] = [];
const moveWindow = (x: number, y: number) => {
  Object.assign(win, { x, y, max: false });
  emit("tauri://move", { x, y });
};
window.__mock = { set, emit, status: () => status, calls, moveWindow };

const w = window as unknown as Record<string, unknown>;
w.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
  transformCallback: (cb: (p: unknown) => void) => {
    const id = nextId++;
    callbacks.set(id, cb);
    return id;
  },
  unregisterCallback: (id: number) => callbacks.delete(id),
  convertFileSrc: (p: string) => p,
  invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
    calls.push(Object.keys(args).length && !cmd.startsWith("plugin:") ? `${cmd} ${JSON.stringify(args)}` : cmd);
    if (q.has("old") && (cmd === "set_start_display_on_connect" || cmd === "set_ctrl_as_cmd")) throw `Command ${cmd} not found`;
    const reply = replies[cmd];
    return reply ? reply(args) : undefined;
  },
};
w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: (event: string, id: number) => listeners.get(event)?.delete(id),
};

// The live round trip, as the app reports it every second.
window.setInterval(() => {
  if (status.connection === "connected" && status.latency_ms != null) set({ latency_ms: 2 + Math.floor(Math.random() * 4) });
}, 1000);

export {};

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

export type Side = "left" | "right" | "top" | "bottom";
export type WindowMode = "windowed" | "borderless";

export type Settings = {
  width: number;
  height: number;
  fps: number;
  bitrate_mbps: number;
  window_mode: WindowMode;
  monitor: number | null;
};

export type Peer = {
  key: string;
  name: string;
  host: string;
  addresses: string[];
  current: boolean;
  allowed: boolean;
  paired: boolean;
  connected: boolean;
};

export type Placement = { side: Side; monitor: number | null };
export type Uses = { mouse: boolean; clipboard: boolean; display: boolean };
export type Pairing = { code: string; seconds_left: number };

export type Connection =
  | "off"
  | "connecting"
  | "connected"
  | "lost"
  | "ended_by_mac"
  | "busy"
  | "blocked"
  | "unknown"
  | "version";

export type Status = {
  role: "client" | "host";
  state: "disconnected" | "connecting" | "connected" | "failed";
  part: string | null;
  reason: string | null;
  peer_name: string | null;
  peer_host: string | null;
  paired: boolean;
  settings: Settings;
  config_error: string | null;
  mouse_sharing: boolean;
  mac_side: Side;
  mouse_state: "off" | "running" | "failed";
  mouse_reason: string | null;
  connection: Connection;
  busy_by: string | null;
  peers: Peer[];
  auto_connect: boolean;
  placement: Placement;
  uses: Uses | null;
  pairing: Pairing | null;
  close_to_tray: boolean;
  clipboard_sharing: boolean;
  language_follow: boolean;
  peer_addresses: string[];
  /** Added by Task 2. */
  latency_ms?: number | null;
  /** PC: the monitor the Mac display opens on (Windows only). */
  viewer_monitor?: number | null;
  /** PC: a connect also starts the display; Ctrl is sent as Command while the Mac has the
   *  keyboard. Absent from a backend without them (the switches then show disabled). */
  start_display_on_connect?: boolean;
  ctrl_as_cmd?: boolean;
  /** PC: the wheel scrolls the Mac the other way round (missing from an older app). */
  reverse_scroll?: boolean;
  mac_sound?: boolean;
};

export type SystemInfo = {
  missing: string | null;
  start_at_login: boolean;
  screen_recording: boolean | null;
  accessibility: boolean | null;
};

/** `number` is added by Task 2. */
export type Monitor = { left: number; top: number; right: number; bottom: number; number: number };
export type Found = { name: string; host: string; id: string; pairing: boolean };
export type SettingsInput = {
  width: number;
  height: number;
  fps: number;
  bitrate_mbps: number;
  window_mode: WindowMode;
  monitor: number | null;
  /** The user picked `monitor` with this change (a monitor button): a restart goes there even
   *  when the display is a window dragged to another monitor. */
  monitor_picked?: boolean;
};
/** `in_place`: `installUpdate` installs `latest` and restarts (the PC); otherwise `url` has it. */
export type UpdateInfo = { current: string; latest: string | null; url: string; in_place: boolean };

// connection
export const getStatus = () => invoke<Status>("get_status");
export const connectMac = (key?: string, code?: string) => invoke<void>("connect_mac", { key, code });
export const disconnectMac = () => invoke<void>("disconnect_mac");
/** Added by Task 2. */
export const reconnectMac = () => invoke<void>("reconnect_mac");

// display
export const displayStart = () => invoke<void>("display_start");
export const displayStop = () => invoke<void>("display_stop");
/** The display window (Windows only): fill a monitor (by number; the current one when null), go
 *  back to a window (on a monitor, by number; the current one when null), minimize. */
export const displayFullScreen = (monitor: number | null = null) => invoke<void>("display_full_screen", { monitor });
export const displayWindow = (monitor: number | null = null) => invoke<void>("display_window", { monitor });
export const displayMinimize = () => invoke<void>("display_minimize");
/** The bar was pulled down: the display becomes a window under the pointer, following it until
 *  the button is let go. */
export const displayPullDown = () => invoke<void>("display_pull_down");
/** Sent when the display window is shown or changes mode. */
export type DisplayMode = { full_screen: boolean; monitor: number };

// the bar over the full-screen display (Windows only)
/** What of the bar shows: nothing, the tab, the open bar, or the whole window (a menu or popover
 *  under the bar). Only that takes the pointer. */
export type BarShape = "hidden" | "bar" | "menu";
/** Sets what of the bar shows (for "bar": the open bar's own size in CSS pixels, and whether one
 *  of its tooltips shows, which gets room round the bar). */
export const barShape = (shape: BarShape, width?: number, height?: number, tip?: boolean) =>
  invoke<void>("bar_shape", { shape, width, height, tip });
/** A menu or popover of the bar opened or closed: the bar has the keyboard while it is open. */
export const barMenuOpen = (open: boolean) => invoke<void>("bar_menu_open", { open });
/** The display window's title bar settings button (its rect, CSS pixels in the display window):
 *  the display settings popover opens under it, in the bar's window. */
export const barSettings = (r: { left: number; top: number; right: number; bottom: number }) =>
  invoke<void>("bar_settings", { left: r.left, top: r.top, right: r.right, bottom: r.bottom });

// settings and sharing
export const saveSettings = (settings: SettingsInput) => invoke<void>("save_settings", { settings });
export const setMouseSharing = (enabled: boolean) => invoke<void>("set_mouse_sharing", { enabled });
export const setClipboardSharing = (enabled: boolean) => invoke<void>("set_clipboard_sharing", { enabled });
export const setLanguageFollow = (enabled: boolean) => invoke<void>("set_language_follow", { enabled });
export const setPlacement = (side: Side, monitor: number | null) => invoke<void>("set_placement", { side, monitor });
export const setStartDisplayOnConnect = (enabled: boolean) => invoke<void>("set_start_display_on_connect", { enabled });
export const setCtrlAsCmd = (enabled: boolean) => invoke<void>("set_ctrl_as_cmd", { enabled });
export const setReverseScroll = (enabled: boolean) => invoke<void>("set_reverse_scroll", { enabled });
export const setMacSound = (enabled: boolean) => invoke<void>("set_mac_sound", { enabled });
/** The backend has no such command (one older than the switch calling it): Tauri's own rejection,
 *  `Command <name> not found`, and nothing else. */
export const isMissingCommand = (e: unknown) => typeof e === "string" && /^Command [\w:|-]+ not found$/.test(e);

// Macs
export const findMacs = () => invoke<Found[]>("find_macs");
export const addMacByAddress = (host: string, code: string) => invoke<void>("add_mac_by_address", { host, code });
/** Added by Task 2. */
/** Resolves to the Mac's key once at that address (a Mac known by address alone is keyed by it). */
export const setMacAddress = (key: string, host: string) => invoke<string>("set_mac_address", { key, host });
export const forgetMac = (key: string) => invoke<void>("forget_mac", { key });

// app
export const setAutoConnect = (enabled: boolean) => invoke<void>("set_auto_connect", { enabled });
export const setCloseToTray = (enabled: boolean) => invoke<void>("set_close_to_tray", { enabled });
export const getSystem = () => invoke<SystemInfo>("get_system");
export const setStartAtLogin = (enabled: boolean) => invoke<void>("set_start_at_login", { enabled });
export const installMissing = () => invoke<void>("install_missing");
export const openPrivacySettings = (kind: "screen" | "accessibility") => invoke<void>("open_privacy_settings", { kind });
export const closeWindow = () => invoke<void>("close_window");
export const resetConfig = () => invoke<void>("reset_config");

// Windows only (Task 2)
export const getMonitors = () => invoke<Monitor[]>("get_monitors");
export const backdrop = () => invoke<"mica" | "solid">("backdrop");
export const checkUpdate = () => invoke<UpdateInfo>("check_update");
/** Downloads, installs and restarts; resolves only if that fails to start. */
export const installUpdate = () => invoke<void>("install_update");
export const openUrl = (url: string) => invoke<void>("open_url", { url });

export const onStatus = (cb: (s: Status) => void): Promise<UnlistenFn> =>
  listen<Status>("state-changed", (e) => cb(e.payload));

/** The status: fetched once, then following `state-changed`. */
export function useStatus(): Status | null {
  const [status, setStatus] = useState<Status | null>(null);
  useEffect(() => {
    let alive = true;
    let unlisten: UnlistenFn | undefined;
    void onStatus((s) => alive && setStatus(s)).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    getStatus()
      .then((s) => alive && setStatus((prev) => prev ?? s))
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);
  return status;
}

/** The system info, polled every 2 s. */
export function useSystem(): { system: SystemInfo | null; refresh: () => Promise<void> } {
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const refresh = async () => {
    try {
      setSystem(await getSystem());
    } catch {
      /* keep the last value */
    }
  };
  useEffect(() => {
    let alive = true;
    const tick = () =>
      getSystem()
        .then((s) => alive && setSystem(s))
        .catch(() => {});
    tick();
    const id = window.setInterval(tick, 2000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);
  return { system, refresh };
}

import { displayFullScreen, displayStart, displayWindow, saveSettings, type Status } from "../api";

// One start of the display at a time, whoever asks (the tile, a monitor's Show here).
let starting = false;
/** True when this caller may start the display; it calls releaseStart() when done. */
export function claimStart(): boolean {
  if (starting) return false;
  starting = true;
  return true;
}
export function releaseStart() {
  starting = false;
}

/** Shows the Mac's display on monitor `n` (a monitor's Show here on the desk). A running display
 *  moves there in its current mode, without a restart; otherwise `n` is saved as the display's
 *  monitor and the display starts. */
export async function showDisplayOn(status: Status, n: number): Promise<void> {
  const s = status.settings;
  if (status.state === "connected" || status.state === "connecting") {
    await (s.window_mode === "windowed" ? displayWindow(n) : displayFullScreen(n));
    return;
  }
  if (!claimStart()) return;
  try {
    await saveSettings({
      width: s.width, height: s.height, fps: s.fps, bitrate_mbps: s.bitrate_mbps, window_mode: s.window_mode, monitor: n, monitor_picked: true,
    });
    await displayStart();
  } finally {
    releaseStart();
  }
}

import { Menu } from "@tauri-apps/api/menu";
import {
  displayFullScreen, displayStop, displayWindow, getMonitors, saveSettings, type DisplayMode, type SettingsInput, type Status,
} from "../api";
import { bitrateOptions, fpsLabel, fpsOptions, rateLabel, resLabel, resolutionOptions } from "../pc/displayOptions";

const quietly = (p: Promise<unknown>) => void p.catch(() => {});

/** The display window's menu (a right click on its title bar): Resolution, Frame rate and Bitrate
 *  (as in the bar), Monitor, Full screen and Close display. A native menu: the Mac's display
 *  covers the page below the 40px title bar, so nothing the page draws could show there. */
export async function showDisplayMenu(status: Status | null, mode: DisplayMode | null) {
  const s = status?.settings;
  if (!s) return;
  const monitors = await getMonitors().catch(() => []);
  const current = (mode && mode.monitor > 0 ? mode.monitor : null) ?? status?.viewer_monitor ?? s.monitor ?? null;
  const cur = { w: s.width, h: s.height };
  // A setting restarts the stream with it; the mode and the monitor stay as they are.
  const save = (patch: Partial<SettingsInput>) =>
    quietly(
      saveSettings({
        width: s.width, height: s.height, fps: s.fps, bitrate_mbps: s.bitrate_mbps, window_mode: s.window_mode,
        monitor: s.monitor, ...patch,
      }),
    );
  const menu = await Menu.new({
    items: [
      {
        text: "Resolution",
        items: resolutionOptions(cur).map((r) => ({
          text: resLabel(r),
          checked: r.w === s.width && r.h === s.height,
          action: () => {
            if (r.w !== s.width || r.h !== s.height) save({ width: r.w, height: r.h });
          },
        })),
      },
      {
        text: "Frame rate",
        items: fpsOptions(s.fps).map((v) => ({
          text: fpsLabel(v),
          checked: v === s.fps,
          action: () => {
            if (v !== s.fps) save({ fps: v });
          },
        })),
      },
      {
        text: "Bitrate",
        items: bitrateOptions(s.bitrate_mbps).map((v) => ({
          text: rateLabel(v),
          checked: v === s.bitrate_mbps,
          action: () => {
            if (v !== s.bitrate_mbps) save({ bitrate_mbps: v });
          },
        })),
      },
      { item: "Separator" },
      {
        text: "Monitor",
        items: monitors.map((m) => ({
          text: `${m.number} · ${m.right - m.left} × ${m.bottom - m.top}`,
          checked: m.number === current,
          // The window moves to that monitor (it comes back where it last was there).
          action: () => {
            if (m.number !== current) quietly(displayWindow(m.number));
          },
        })),
      },
      { text: "Full screen", action: () => quietly(displayFullScreen()) },
      { item: "Separator" },
      { text: "Close display", action: () => quietly(displayStop()) },
    ],
  });
  await menu.popup();
  // Its items are the app's until it goes; a chosen one has been acted on by then. Their ids are
  // the app's own (none given), so this never takes a newer menu's items with it.
  window.setTimeout(() => quietly(menu.close()), 5000);
}

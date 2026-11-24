import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { FullScreenMaximize16Regular, Settings16Regular } from "@fluentui/react-icons";
import { barSettings, displayFullScreen, displayMinimize, displayStop, type DisplayMode, type Status } from "../api";
import { showDisplayMenu } from "./displayMenu";

const run = (action: () => Promise<unknown>) => () => void action().catch(() => {});

/** The display window's mode and monitor, as the app last told. */
function useDisplayMode(): DisplayMode | null {
  const [mode, setMode] = useState<DisplayMode | null>(null);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void listen<DisplayMode>("display-mode", (e) => alive && setMode(e.payload)).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);
  return mode;
}

/** The display window's title bar: the Mac's name and the display data, then a display settings
 *  button and the caption buttons for Full screen, Minimize and Close (which closes the
 *  display). A right click on it opens the display's menu. Only this 40px strip of the page can
 *  be seen (the Mac's display covers the rest), so the settings popover opens in the bar's window
 *  under its button, and the menu is a native one. */
export function DisplayTitleBar({ status }: { status: Status | null }) {
  const s = status?.settings;
  const mode = useDisplayMode();
  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    void showDisplayMenu(status, mode).catch(() => {});
  };
  const settings = (e: React.MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    void barSettings({ left: r.left, top: r.top, right: r.right, bottom: r.bottom }).catch(() => {});
  };
  return (
    <div className="ss-dtb" data-tauri-drag-region onContextMenu={menu}>
      <div className="ss-dtb-title" data-tauri-drag-region>
        <img className="ss-dtb-icon" src="/icon.png" alt="" width={16} height={16} draggable={false} />
        <span className="ss-dtb-name" data-tauri-drag-region>{status?.peer_name ?? "Sidescreen"}</span>
        {s && (
          <span className="ss-dtb-data" data-tauri-drag-region>
            {s.width} × {s.height} · {s.fps} fps
          </span>
        )}
      </div>
      <div className="ss-dtb-fill" data-tauri-drag-region />
      <div className="ss-dtb-caps">
        <button type="button" className="ss-dtb-cap" aria-label="Display settings" onClick={settings}>
          <Settings16Regular className="ss-dtb-glyph" />
        </button>
        <button type="button" className="ss-dtb-cap" aria-label="Full screen" onClick={run(() => displayFullScreen())}>
          <FullScreenMaximize16Regular className="ss-dtb-glyph" />
        </button>
        <button type="button" className="ss-dtb-cap" aria-label="Minimize" onClick={run(displayMinimize)}>
          <svg viewBox="0 0 10 10"><path d="M0 5.5h10" /></svg>
        </button>
        <button type="button" className="ss-dtb-cap ss-dtb-close" aria-label="Close display" onClick={run(displayStop)}>
          <svg viewBox="0 0 10 10"><path d="M.5.5l9 9M9.5.5l-9 9" /></svg>
        </button>
      </div>
    </div>
  );
}

import { getCurrentWindow } from "@tauri-apps/api/window";

const run = (action: () => Promise<void>) => () => void action().catch(() => {});

export function TitleBar() {
  const win = getCurrentWindow();
  return (
    <div className="ss-titlebar">
      <div className="ss-tb-title" data-tauri-drag-region>
        <img className="ss-appicon" src="/icon.png" alt="" width={16} height={16} draggable={false} />
        <span data-tauri-drag-region>Sidescreen</span>
      </div>
      <div className="ss-tb-fill" data-tauri-drag-region />
      <div className="ss-caps">
        <button type="button" className="ss-cap" aria-label="Minimize" onClick={run(() => win.minimize())}>
          <svg viewBox="0 0 10 10"><path d="M0 5.5h10" /></svg>
        </button>
        <button type="button" className="ss-cap" aria-label="Maximize" onClick={run(() => win.toggleMaximize())}>
          <svg viewBox="0 0 10 10"><rect x=".5" y=".5" width="9" height="9" rx="1.5" /></svg>
        </button>
        <button type="button" className="ss-cap ss-close" aria-label="Close" onClick={run(() => win.close())}>
          <svg viewBox="0 0 10 10"><path d="M.5.5l9 9M9.5.5l-9 9" /></svg>
        </button>
      </div>
    </div>
  );
}

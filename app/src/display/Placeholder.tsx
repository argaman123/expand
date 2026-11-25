import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Button, Spinner } from "@fluentui/react-components";
import { displayStart, displayStop, type DisplayMode, type Status } from "../api";
import "./placeholder.css";

/** Follows a Tauri event while mounted. */
function useEventValue<T>(name: string, initial: T): T {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void listen<T>(name, (e) => alive && setValue(e.payload)).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [name]);
  return value;
}

/** What shows in the display window where the Mac's display goes, while it is not there: Starting,
 *  or why the display failed, with Retry and Close. Under the title bar, or over all of the window
 *  when it is full screen. The Mac's display, once in, covers it. */
export function Placeholder({ status }: { status: Status | null }) {
  const mode = useEventValue<DisplayMode | null>("display-mode", null);
  const viewer = useEventValue<boolean>("display-viewer", false);
  const [error, setError] = useState<string | null>(null);
  const failed = status?.state === "failed";
  useEffect(() => setError(null), [status?.state]);

  const retry = () => {
    setError(null);
    displayStart().catch((e) => setError(String(e)));
  };

  return (
    <div className={`ex-ph${mode?.full_screen ? " ex-ph-full" : ""}`} aria-live="polite">
      {viewer ? null : failed ? (
        <div className="ex-ph-fail">
          <span className="ex-ph-part">{status?.part ?? "Display"}</span>
          <span className="ex-ph-reason">{error ?? status?.reason ?? "failed"}</span>
          <div className="ex-ph-acts">
            <Button appearance="primary" size="small" onClick={retry}>Retry</Button>
            <Button size="small" onClick={() => void displayStop().catch(() => {})}>Close</Button>
          </div>
        </div>
      ) : (
        <div className="ex-ph-start">
          <Spinner size="tiny" />
          <span>Starting</span>
        </div>
      )}
    </div>
  );
}

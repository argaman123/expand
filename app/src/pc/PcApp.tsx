import { useCallback, useEffect, useRef, useState } from "react";
import {
  Button, FluentProvider, MessageBar, MessageBarActions, MessageBarBody, MessageBarTitle, Spinner, Toast, ToastBody,
  ToastFooter, ToastTitle, Toaster, useId, useToastController,
} from "@fluentui/react-components";
import { backdrop, installMissing, resetConfig, useSystem, type Status, type SystemInfo } from "../api";
import { THEME } from "./theme";
import { TitleBar } from "./TitleBar";
import { Header } from "./Header";
import { Desk } from "./Desk";
import { Controls } from "./Controls";
import { SettingsDrawer } from "./SettingsDrawer";
import { useNotices, type Notice } from "./useNotices";
import { ErrorLine, failureText, type Failure } from "./ErrorLine";
import "../brand.css";
import "./pc.css";

/** The screens' wallpaper drifts only while someone can see it: `data-still` on <html> while the
 *  window is hidden or not focused (pc.css pauses it then). Events only. */
function useStillWhenAway() {
  useEffect(() => {
    const root = document.documentElement;
    const still = () => {
      if (document.visibilityState === "hidden" || !document.hasFocus()) root.dataset.still = "";
      else delete root.dataset.still;
    };
    still();
    document.addEventListener("visibilitychange", still);
    window.addEventListener("blur", still);
    window.addEventListener("focus", still);
    return () => {
      document.removeEventListener("visibilitychange", still);
      window.removeEventListener("blur", still);
      window.removeEventListener("focus", still);
    };
  }, []);
}

function useBackdrop(): "mica" | "solid" {
  const [kind, setKind] = useState<"mica" | "solid">("solid");
  useEffect(() => {
    backdrop().then(setKind).catch(() => {});
  }, []);
  return kind;
}

const BANNER_PX = 40;

/** The PC window. Its frame (theme, backdrop, title bar) is there before the status arrives, so
 *  the frameless window can always be seen, moved and closed. */
export function PcApp({ status }: { status: Status | null }) {
  const kind = useBackdrop();
  useStillWhenAway();
  const { system, refresh } = useSystem();
  const [winEl, setWinEl] = useState<HTMLDivElement | null>(null);
  const banners = (system?.missing ? 1 : 0) + (status?.config_error ? 1 : 0);
  return (
    <FluentProvider theme={THEME} className="ss-provider">
      <div className={`ss-win ${kind}`} ref={setWinEl} style={{ "--ss-banner": `${banners * BANNER_PX}px` } as React.CSSProperties}>
        <TitleBar />
        {status ? (
          <PcBody status={status} system={system} refreshSystem={refresh} winEl={winEl} />
        ) : (
          <div className="ss-loading"><Spinner size="small" /></div>
        )}
      </div>
    </FluentProvider>
  );
}

function PcBody({ status, system, refreshSystem, winEl }: {
  status: Status;
  system: SystemInfo | null;
  refreshSystem: () => Promise<void>;
  winEl: HTMLDivElement | null;
}) {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [actionError, setActionError] = useState<Failure | null>(null);
  const toasterId = useId("toaster");
  const { dispatchToast, dismissToast } = useToastController(toasterId);
  const toastSeq = useRef(0);
  const slotToast = useRef(new Map<string, string>());
  const closeSettings = useCallback(() => setSettingsOpen(false), []);

  useNotices(status, (n: Notice) => {
    const id = `toast-${++toastSeq.current}`;
    if (n.slot) {
      const old = slotToast.current.get(n.slot);
      if (old) dismissToast(old); // a restart replaces the old values
      slotToast.current.set(n.slot, id);
    }
    dispatchToast(
      <Toast className={`ss-toast ss-toast-${n.intent}`}>
        <ToastTitle>{n.title}</ToastTitle>
        {n.body && <ToastBody>{n.body}</ToastBody>}
        {n.action && (
          <ToastFooter>
            <Button size="small" appearance="transparent" onClick={n.action.run}>{n.action.label}</Button>
          </ToastFooter>
        )}
      </Toast>,
      { toastId: id, intent: n.intent, position: "bottom-end", timeout: n.action ? 6000 : 2600 },
    );
  });

  const run = (action: () => Promise<unknown>) => {
    setActionError(null);
    action().catch((e) => setActionError({ text: failureText(e), retry: () => run(action) }));
  };
  const banners = (system?.missing ? 1 : 0) + (status.config_error ? 1 : 0);
  return (
    <>
      {banners > 0 && (
        <div className="ss-banners">
          {system?.missing && (
            <MessageBar intent="warning">
              <MessageBarBody><MessageBarTitle>Moonlight missing</MessageBarTitle></MessageBarBody>
              <MessageBarActions>
                <Button size="small" onClick={() => run(installMissing)}>Install</Button>
              </MessageBarActions>
            </MessageBar>
          )}
          {status.config_error && (
            <MessageBar intent="error">
              <MessageBarBody><MessageBarTitle title={status.config_error}>{status.config_error}</MessageBarTitle></MessageBarBody>
              <MessageBarActions>
                <Button size="small" onClick={() => run(resetConfig)}>Reset settings</Button>
              </MessageBarActions>
            </MessageBar>
          )}
        </div>
      )}
      <Header status={status} onOpenSettings={() => setSettingsOpen(true)} />
      <div className="ss-slot-desk"><Desk status={status} /></div>
      <div className="ss-slot-controls"><Controls status={status} /></div>
      {actionError && <ErrorLine className="ss-header-err ss-action-err" failure={actionError} />}
      <SettingsDrawer open={settingsOpen} close={closeSettings} status={status} system={system} refreshSystem={refreshSystem} />
      {winEl && <Toaster toasterId={toasterId} mountNode={winEl} offset={{ horizontal: 24, vertical: 24 }} />}
    </>
  );
}

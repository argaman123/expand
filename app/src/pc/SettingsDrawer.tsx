import { useEffect, useId, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Button, Divider, DrawerBody, DrawerHeader, DrawerHeaderTitle, InlineDrawer, Spinner, Switch } from "@fluentui/react-components";
import { Dismiss24Regular } from "@fluentui/react-icons";
import {
  checkUpdate, installUpdate, isMissingCommand, openUrl, setAutoConnect, setCloseToTray, setStartAtLogin, setStartDisplayOnConnect,
  type Status, type SystemInfo, type UpdateInfo,
} from "../api";
import { ErrorLine, type Failure } from "./ErrorLine";

type Check =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "done"; info: UpdateInfo }
  | { kind: "installing" }
  | { kind: "error"; text: string };

const message = (e: unknown) => String(e instanceof Error ? e.message : e);

export function SettingsDrawer({
  open, close, status, system, refreshSystem,
}: {
  open: boolean;
  close: () => void;
  status: Status;
  system: SystemInfo | null;
  refreshSystem: () => Promise<void>;
}) {
  const ids = { auto: useId(), display: useId(), login: useId(), tray: useId() };
  const [version, setVersion] = useState("");
  const [check, setCheck] = useState<Check>({ kind: "idle" });
  const [error, setError] = useState<Failure | null>(null);
  const [noStartDisplay, setNoStartDisplay] = useState(false); // the backend has no such setting

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);
  const drawer = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const before = document.activeElement as HTMLElement | null;
    const focusables = () =>
      [...(drawer.current?.querySelectorAll<HTMLElement>("button, input, [href], [tabindex]:not([tabindex='-1'])") ?? [])].filter(
        (el) => !(el as HTMLButtonElement).disabled,
      );
    focusables()[0]?.focus({ preventScroll: true }); // the drawer is still sliding in, beyond the window
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") return close();
      if (e.key !== "Tab") return;
      const all = focusables();
      if (!all.length) return;
      const first = all[0];
      const last = all[all.length - 1];
      const at = document.activeElement as HTMLElement | null;
      const inside = !!at && !!drawer.current?.contains(at);
      if (e.shiftKey && (at === first || !inside)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (at === last || !inside)) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      before?.focus?.();
    };
  }, [open, close]);

  const change = (run: () => Promise<unknown>) => {
    setError(null);
    run().catch((e) => setError({ text: message(e), retry: () => change(run) }));
  };
  const changeStartDisplay = (on: boolean) => {
    setError(null);
    setStartDisplayOnConnect(on).catch((e) => {
      if (isMissingCommand(e)) setNoStartDisplay(true);
      else setError({ text: message(e), retry: () => changeStartDisplay(on) });
    });
  };
  const runCheck = () => {
    setCheck({ kind: "checking" });
    checkUpdate()
      .then((info) => setCheck({ kind: "done", info }))
      .catch((e) => setCheck({ kind: "error", text: message(e) }));
  };
  // The app closes and the installer starts it again, so this returns only on a failure.
  const runInstall = () => {
    setCheck({ kind: "installing" });
    installUpdate().catch((e) => setCheck({ kind: "error", text: message(e) }));
  };

  let update: JSX.Element;
  if (check.kind === "checking") update = <Spinner size="tiny" aria-label="Checking for updates" />;
  else if (check.kind === "installing") update = <Spinner size="tiny" aria-label="Updating" />;
  else if (check.kind === "done" && check.info.latest) {
    const { latest, url, in_place } = check.info;
    update = in_place ? (
      <Button size="small" appearance="primary" onClick={runInstall}>{`Update to ${latest}`}</Button>
    ) : (
      <Button size="small" appearance="primary" onClick={() => change(() => openUrl(url))}>{`Download ${latest}`}</Button>
    );
  } else if (check.kind === "done") update = <span>Up to date</span>;
  else update = <Button size="small" onClick={runCheck}>{check.kind === "error" ? "Retry" : "Check for updates"}</Button>;

  // A failed update check has its Retry in the About row already.
  const checkError = check.kind === "error" ? check.text : null;

  return (
    <>
      <div className={`ex-scrim${open ? " on" : ""}`} onClick={close} />
      <InlineDrawer open={open} position="end" size="small" className="ex-drawer" ref={drawer}>
        <DrawerHeader>
          <DrawerHeaderTitle action={<Button appearance="subtle" aria-label="Close" icon={<Dismiss24Regular />} onClick={close} />}>
            Settings
          </DrawerHeaderTitle>
        </DrawerHeader>
        <DrawerBody className="ex-dbody">
          <section>
            <h4>App</h4>
            <div className="ex-card">
              <div className="ex-cardrow">
                <label htmlFor={ids.auto}>Connect automatically</label>
                <Switch id={ids.auto} checked={status.auto_connect} onChange={(_, d) => change(() => setAutoConnect(d.checked))} />
              </div>
              <Divider />
              <div className="ex-cardrow">
                <label htmlFor={ids.display}>Start the display when connected</label>
                <Switch
                  id={ids.display}
                  checked={status.start_display_on_connect ?? false}
                  disabled={status.start_display_on_connect === undefined || noStartDisplay}
                  onChange={(_, d) => changeStartDisplay(d.checked)}
                />
              </div>
              <Divider />
              <div className="ex-cardrow">
                <label htmlFor={ids.login}>Start when I sign in</label>
                <Switch
                  id={ids.login}
                  checked={system?.start_at_login ?? false}
                  disabled={!system}
                  onChange={(_, d) => change(() => setStartAtLogin(d.checked).then(refreshSystem))}
                />
              </div>
              <Divider />
              <div className="ex-cardrow">
                <label htmlFor={ids.tray}>Keep running in the tray</label>
                <Switch id={ids.tray} checked={status.close_to_tray} onChange={(_, d) => change(() => setCloseToTray(d.checked))} />
              </div>
            </div>
          </section>
          <section>
            <h4>About</h4>
            <div className="ex-card">
              <div className="ex-cardrow">
                <span>{`Expand ${version}`.trim()}</span>
                {update}
              </div>
            </div>
          </section>
          {error ? (
            <ErrorLine className="ex-header-err ex-drawer-err" failure={error} />
          ) : checkError && <div className="ex-header-err ex-drawer-err" title={checkError}>{checkError}</div>}
        </DrawerBody>
      </InlineDrawer>
    </>
  );
}

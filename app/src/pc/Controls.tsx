import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { Dropdown, Link, Option, Spinner, Switch, ToggleButton } from "@fluentui/react-components";
import { Desktop20Regular, KeyboardMouse20Regular } from "@fluentui/react-icons";
import {
  connectMac, displayFullScreen, displayStart, displayStop, displayWindow, isMissingCommand, saveSettings, setClipboardSharing, setCtrlAsCmd, setMacSound, setReverseScroll,
  setLanguageFollow, setMouseSharing, type Settings, type Status, type WindowMode,
} from "../api";
import { HoverPanel } from "./HoverPanel";
import { claimStart, releaseStart } from "./startGuard";
import { useMonitors } from "./useMonitors";
import { ErrorLine, failureText, type Failure } from "./ErrorLine";
import { bitrateOptions, fpsLabel, fpsOptions, rateLabel, resKey, resLabel, resolutionOptions } from "./displayOptions";

function Row({ label, id, children }: { label: string; id?: string; children: ReactNode }) {
  return <div className="ex-row"><label htmlFor={id}>{label}</label>{children}</div>;
}

const CONNECT_WAIT_MS = 10_000;

export function Controls({ status }: { status: Status }) {
  const ids = { res: useId(), fps: useId(), rate: useId(), clip: useId(), lang: useId(), cmd: useId(), scroll: useId(), sound: useId() };
  const monitors = useMonitors();
  const [panel, setPanel] = useState<"mouse" | "display" | null>(null); // the hover panel showing: one at a time
  const [error, setError] = useState<Failure | null>(null);
  const [noCtrlAsCmd, setNoCtrlAsCmd] = useState(false); // the backend has no such setting
  const [waiting, setWaiting] = useState(false); // connecting to the Mac before the display starts
  const latest = useRef(status);
  latest.current = status;
  const alive = useRef(true);
  const watchers = useRef(new Set<(cancel?: boolean) => void>());
  const pendingSave = useRef<Settings | null>(null);
  const isConnected = () => latest.current.connection === "connected";
  useEffect(() => {
    alive.current = true;
    const w = watchers.current;
    return () => {
      alive.current = false;
      [...w].forEach((f) => f(true)); // ends every wait (and its timer)
    };
  }, []);
  useEffect(() => {
    [...watchers.current].forEach((f) => f());
  }, [status]);
  useEffect(() => {
    pendingSave.current = null; // the status now carries the saved settings
  }, [status.settings]);

  // A failure shows with a Retry that runs the same action again.
  const fail = (retry: () => void) => (e: unknown) => {
    if (alive.current) setError({ text: failureText(e), retry });
  };
  const attempt = (run: () => Promise<unknown>) => {
    setError(null);
    run().catch(fail(() => attempt(run)));
  };
  const changeCtrlAsCmd = (on: boolean) => {
    setError(null);
    setCtrlAsCmd(on).catch((e) => {
      if (isMissingCommand(e)) setNoCtrlAsCmd(true);
      else fail(() => changeCtrlAsCmd(on))(e);
    });
  };
  const s = status.settings;
  const mouseOn = status.mouse_sharing;
  // The display session alone decides the tile: the link may drop while the stream runs on.
  const connected = status.state === "connected";
  const running = status.state === "connected" || status.state === "connecting";
  const starting = status.state === "connecting" || waiting;
  const failed = status.state === "failed" && !waiting;
  const currentKey = status.peers.find((p) => p.current)?.key;

  // Resolves true once pred() holds (checked on every status change), false after ms or on unmount.
  const waitFor = (pred: () => boolean, ms: number) =>
    new Promise<boolean>((resolve) => {
      if (pred()) return resolve(true);
      const t = window.setTimeout(() => finish(false), ms);
      const entry = (cancel?: boolean) => {
        if (cancel) finish(false);
        else if (pred()) finish(true);
      };
      const finish = (v: boolean) => {
        window.clearTimeout(t);
        watchers.current.delete(entry);
        resolve(v);
      };
      watchers.current.add(entry);
    });

  const start = async () => {
    if (!claimStart()) return;
    setError(null);
    setWaiting(true);
    try {
      if (!isConnected()) {
        await connectMac(currentKey);
        if (!(await waitFor(isConnected, CONNECT_WAIT_MS))) {
          if (alive.current) throw new Error("Not connected");
          return;
        }
      }
      await displayStart();
      await waitFor(() => latest.current.state !== "disconnected", 3000); // no Off flash before the status catches up
    } catch (e) {
      fail(() => void start())(e);
    } finally {
      releaseStart();
      if (alive.current) setWaiting(false);
    }
  };
  const stop = () => attempt(displayStop);
  const toggleDisplay = () => {
    if (running) stop();
    else if (!starting) void start();
  };

  // Where the display opens, as the backend resolves it (a chosen monitor that was unplugged
  // resolves to another); that is what is shown and saved.
  const selectedMonitor = (st: Status) => st.viewer_monitor ?? st.settings.monitor ?? null;
  const save = (patch: Partial<Settings>) => {
    setError(null);
    const base = pendingSave.current ?? { ...latest.current.settings, monitor: selectedMonitor(latest.current) };
    const m = { ...base, ...patch };
    pendingSave.current = m;
    saveSettings({
      width: m.width, height: m.height, fps: m.fps, bitrate_mbps: m.bitrate_mbps, window_mode: m.window_mode, monitor: m.monitor,
      monitor_picked: patch.monitor !== undefined,
    }).catch((e) => {
      pendingSave.current = null;
      fail(() => save(patch))(e);
    });
  };

  // The mode alone never restarts the stream, whatever the display is doing (starting, winding
  // down, off): the commands change a display that shows right away and otherwise only save it.
  const setMode = (mode: WindowMode) => {
    setError(null);
    // A settings save still in flight carries the old mode; the next one builds on it, so it
    // must not put that back.
    if (pendingSave.current) pendingSave.current = { ...pendingSave.current, window_mode: mode };
    attemptMode(mode);
  };
  const attemptMode = (mode: WindowMode) => {
    (mode === "windowed" ? displayWindow() : displayFullScreen()).catch(fail(() => attemptMode(mode)));
  };

  // Numbered buttons drawn in the proportions of the real monitors (the spec): a bigger monitor
  // is a bigger button whether or not it is selected.
  const picks = useMemo(() => {
    const dims = monitors.map((m) => ({ n: m.number, w: Math.max(1, m.right - m.left), h: Math.max(1, m.bottom - m.top) }));
    if (!dims.length) return [];
    const k = Math.min(60 / Math.max(...dims.map((d) => d.w)), 34 / Math.max(...dims.map((d) => d.h)));
    return dims.map((d) => ({ n: d.n, w: Math.max(24, Math.round(d.w * k)), h: Math.max(16, Math.round(d.h * k)) }));
  }, [monitors]);
  const selMonitor = selectedMonitor(status);

  const cur = { w: s.width, h: s.height };
  const resOpts = resolutionOptions(cur);

  return (
    <>
      <div className="ex-tilewrap">
        <HoverPanel
          label="Mouse and keyboard settings"
          current={panel === "mouse"}
          onOpen={() => setPanel("mouse")}
          trigger={
            <ToggleButton
              className="ex-tile"
              checked={mouseOn}
              icon={<KeyboardMouse20Regular />}
              aria-label="Mouse and keyboard"
              onClick={() => attempt(() => setMouseSharing(!mouseOn))}
            />
          }
        >
          <div className="ex-ph">
            <KeyboardMouse20Regular /><b>Mouse and keyboard</b>
            <Switch checked={mouseOn} aria-label="Mouse and keyboard" onChange={(_, d) => attempt(() => setMouseSharing(d.checked))} />
          </div>
          <div className="ex-pb">
            <Row label="Clipboard" id={ids.clip}>
              <Switch id={ids.clip} checked={status.clipboard_sharing} onChange={(_, d) => attempt(() => setClipboardSharing(d.checked))} />
            </Row>
            <Row label="Sync keyboard language" id={ids.lang}>
              <Switch id={ids.lang} checked={status.language_follow} onChange={(_, d) => attempt(() => setLanguageFollow(d.checked))} />
            </Row>
            <Row label="Ctrl as ⌘" id={ids.cmd}>
              <Switch
                id={ids.cmd}
                checked={status.ctrl_as_cmd ?? false}
                disabled={status.ctrl_as_cmd === undefined || noCtrlAsCmd}
                onChange={(_, d) => changeCtrlAsCmd(d.checked)}
              />
            </Row>
            <Row label="Reverse scrolling" id={ids.scroll}>
              <Switch
                id={ids.scroll}
                checked={status.reverse_scroll ?? false}
                disabled={status.reverse_scroll === undefined}
                onChange={(_, d) => attempt(() => setReverseScroll(d.checked))}
              />
            </Row>
            {/* Alt is always Option on the Mac (share/keys.rs, and Sunshine in the display): said, as the owner asked (round 4). */}
            <Row label="Option ⌥"><span className="ex-keys"><kbd>Alt</kbd></span></Row>
            <Row label="Return to PC"><span className="ex-keys"><kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>Alt</kbd><kbd>Win</kbd></span></Row>
          </div>
        </HoverPanel>
        <span className="ex-tl">Mouse and keyboard</span>
        <span className="ex-state">{mouseOn ? "On" : "Off"}</span>
      </div>

      <div className="ex-tilewrap">
        <HoverPanel
          label="Extended display settings"
          current={panel === "display"}
          onOpen={() => setPanel("display")}
          trigger={
            <ToggleButton
              className="ex-tile"
              checked={connected}
              aria-label="Extended display"
              onClick={toggleDisplay}
              icon={starting ? <Spinner size="tiny" /> : <Desktop20Regular />}
            />
          }
        >
          <div className="ex-ph">
            <Desktop20Regular /><b>Extended display</b>
            <Switch
              checked={connected || starting}
              aria-label="Extended display"
              onChange={(_, d) => { if (!d.checked) stop(); else if (!connected && !starting) void start(); }}
            />
          </div>
          <div className="ex-pb">
            <Row label="Monitor">
              <span className="ex-monpick">
                {picks.map((m) => (
                  <button
                    key={m.n}
                    className={`ex-mp${selMonitor === m.n ? " sel" : ""}`}
                    style={{ width: m.w, height: m.h }}
                    aria-pressed={selMonitor === m.n}
                    onClick={() => save({ monitor: m.n })}
                  >{m.n}</button>
                ))}
              </span>
            </Row>
            <Row label="Mode">
              <span className="ex-seg">
                {([["windowed", "Window"], ["borderless", "Full screen"]] as [WindowMode, string][]).map(([mode, text]) => (
                  <ToggleButton key={mode} size="small" appearance="subtle" checked={s.window_mode === mode} onClick={() => setMode(mode)}>{text}</ToggleButton>
                ))}
              </span>
            </Row>
            <Row label="Resolution" id={ids.res}>
              <Dropdown
                id={ids.res} inlinePopup size="small" className="ex-dd"
                value={resLabel(cur)} selectedOptions={[resKey(cur)]}
                onOptionSelect={(_, d) => {
                  const r = resOpts.find((x) => resKey(x) === d.optionValue);
                  if (r) save({ width: r.w, height: r.h });
                }}
              >
                {resOpts.map((r) => <Option key={resKey(r)} value={resKey(r)} text={resLabel(r)}>{resLabel(r)}</Option>)}
              </Dropdown>
            </Row>
            <Row label="Frame rate" id={ids.fps}>
              <Dropdown
                id={ids.fps} inlinePopup size="small" className="ex-dd"
                value={fpsLabel(s.fps)} selectedOptions={[String(s.fps)]}
                onOptionSelect={(_, d) => d.optionValue && save({ fps: Number(d.optionValue) })}
              >
                {fpsOptions(s.fps).map((v) => <Option key={v} value={String(v)} text={fpsLabel(v)}>{fpsLabel(v)}</Option>)}
              </Dropdown>
            </Row>
            <Row label="Bitrate" id={ids.rate}>
              <Dropdown
                id={ids.rate} inlinePopup size="small" className="ex-dd"
                value={rateLabel(s.bitrate_mbps)} selectedOptions={[String(s.bitrate_mbps)]}
                onOptionSelect={(_, d) => d.optionValue && save({ bitrate_mbps: Number(d.optionValue) })}
              >
                {bitrateOptions(s.bitrate_mbps).map((v) => <Option key={v} value={String(v)} text={rateLabel(v)}>{rateLabel(v)}</Option>)}
              </Dropdown>
            </Row>
            <Row label="Mac sound" id={ids.sound}>
              <Switch
                id={ids.sound}
                checked={status.mac_sound ?? false}
                disabled={status.mac_sound === undefined}
                onChange={(_, d) => attempt(() => setMacSound(d.checked))}
              />
            </Row>
          </div>
        </HoverPanel>
        <span className="ex-tl">Extended display</span>
        {starting ? (
          <span className="ex-state"><Spinner size="extra-tiny" />Starting</span>
        ) : failed ? (
          <span className="ex-state ex-state-fail" title={`${status.part ?? ""}: ${status.reason ?? ""}`}>
            <span className="ex-state-txt">{status.part ?? "Display"}: {status.reason ?? "failed"}</span>
            <Link as="button" onClick={() => void start()}>Retry</Link>
          </span>
        ) : (
          <span className="ex-state">{connected ? `${s.width} × ${s.height} · ${s.fps} fps` : "Off"}</span>
        )}
      </div>
      {error && <ErrorLine className="ex-ctl-err" failure={error} />}
    </>
  );
}

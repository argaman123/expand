import { useState } from "react";
import { closeWindow, installMissing, openPrivacySettings, resetConfig, setStartAtLogin, useSystem } from "../api";
import type { Status } from "../api";
import "./mac-setup.css";

export function MacSetup({ status }: { status: Status }) {
  const { system, refresh } = useSystem();
  const [installNote, setInstallNote] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);

  const install = async () => {
    try {
      await installMissing();
      setInstalling(true);
      setInstallNote("Installing… follow the window that opened.");
    } catch (e) {
      setInstallNote(String(e));
    }
  };
  const open = (kind: "screen" | "accessibility") => void openPrivacySettings(kind).catch(() => {});

  const missing = system?.missing ?? null;
  const engineSub = installing
    ? "Installing… follow the Terminal window."
    : missing
      ? "Sunshine sends the display to your PC."
      : "Sunshine is installed.";

  return (
    <div className="mac-setup">
      <header className="top">
        <img className="app-icon" src="/icon-mac.png" alt="" width={36} height={36} />
        <div className="titles">
          <h1>Expand</h1>
          <p className="muted">One continuous desktop with your PC</p>
        </div>
      </header>

      {missing && (
        <div className="notice">
          <p>{installNote ?? `${missing} isn't installed. Expand needs it to show the display.`}</p>
          <button className="secondary" onClick={install}>Install</button>
        </div>
      )}
      {status.config_error && (
        <div className="notice">
          <p>{status.config_error}</p>
          <button className="secondary" onClick={() => void resetConfig().catch(() => {})}>Reset settings</button>
        </div>
      )}

      <main>
        <section className="hero">
          <div className="hero-title">Set up this Mac</div>
          <div className="muted small">Three things, once. Each button opens the right place in System Settings.</div>
        </section>
        <ol className="steps">
          <li className="step" data-done={String(system?.screen_recording === true)}>
            <span className="step-mark" />
            <span className="row-text">
              <span className="label">Screen Recording</span>
              <span className="sub">So your PC can show this Mac's extra display.</span>
            </span>
            <button className="secondary" onClick={() => open("screen")}>Open Settings</button>
          </li>
          <li className="step" data-done={String(system?.accessibility === true)}>
            <span className="step-mark" />
            <span className="row-text">
              <span className="label">Accessibility</span>
              <span className="sub">So your PC's mouse and keyboard can control this Mac.</span>
            </span>
            <button className="secondary" onClick={() => open("accessibility")}>Open Settings</button>
          </li>
          <li className="step" data-done={String(!!system && !missing)}>
            <span className="step-mark" />
            <span className="row-text">
              <span className="label">Streaming engine</span>
              <span className="sub">{engineSub}</span>
            </span>
            <button className="secondary" onClick={install}>Install</button>
          </li>
        </ol>
        <div className="card">
          <label className="row">
            <span className="row-text"><span className="label">Start when I log in</span></span>
            <input
              type="checkbox"
              className="switch"
              checked={system?.start_at_login ?? false}
              onChange={async (e) => {
                try {
                  await setStartAtLogin(e.target.checked);
                } finally {
                  await refresh();
                }
              }}
            />
          </label>
        </div>
        <p className="note">When it's done, Expand waits in the menu bar. Open Expand on your PC and press Connect.</p>
        <button className="primary done" onClick={() => void closeWindow()}>Done</button>
      </main>
    </div>
  );
}

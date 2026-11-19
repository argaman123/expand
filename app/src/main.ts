import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Side = "left" | "right" | "top" | "bottom";

type Status = {
  role: "client" | "host";
  state: "disconnected" | "connecting" | "connected" | "failed";
  part: string | null;
  reason: string | null;
  peer_name: string | null;
  peer_host: string | null;
  paired: boolean;
  settings: { width: number; height: number; fps: number; bitrate_mbps: number; window_mode: string };
  config_error: string | null;
  mouse_sharing: boolean;
  mac_side: Side;
  mouse_state: "off" | "running" | "failed";
  mouse_reason: string | null;
  sharing_paused: boolean;
  close_to_tray: boolean;
  clipboard_sharing?: boolean;
  language_follow?: boolean;
  peer_addresses?: string[];
};

/** What kind of link an address is, for showing it. */
function linkKind(address: string): string {
  const [a, b] = address.split(".").map(Number);
  if (a === 100 && b >= 64 && b <= 127) return "Tailscale";
  if (a === 169 && b === 254) return "cable";
  return "network";
}

type System = {
  missing: string | null;
  start_at_login: boolean;
  screen_recording: boolean | null;
  accessibility: boolean | null;
};

type Monitor = { left: number; top: number; right: number; bottom: number };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let current: Status | null = null;
let system: System | null = null;
let monitors: Monitor[] = [];

/** The last error in each area; `render` shows them, so a refresh does not lose them. */
const errors = { toggle: "", settings: "", peer: "", mouse: "" };
const notes = { peer: "" };
type Area = keyof typeof errors;

const idle = (s: Status) => s.state === "disconnected" || s.state === "failed";
const macName = (s: Status) => s.peer_name && s.peer_name !== s.peer_host ? s.peer_name : "your Mac";

// ---------- rendering ----------

function render(s: Status) {
  // "#host" in the address shows the Mac's setup guide anywhere (for checking how it looks).
  if (location.hash === "#host") s = { ...s, role: "host" };
  $("config-error").hidden = !s.config_error;
  $("config-error-text").textContent = s.config_error ?? "";
  $("pc-view").hidden = s.role !== "client";
  $("mac-view").hidden = s.role !== "host";
  if (s.role === "client") renderClient(s);
}

function heroText(s: Status): [string, string] {
  const mac = macName(s);
  switch (s.state) {
    case "connected":
      return [`Connected to ${mac}`, "The Mac's extra display is open on this PC."];
    case "connecting":
      return ["Connecting…", `Starting the display on ${mac}.`];
    case "failed":
      return ["Couldn't connect", ""];
    default:
      if (!s.peer_host) return ["Not set up yet", "Enter your Mac's address below, or find it on the network."];
      return ["Ready", s.paired ? `${mac} · ${s.peer_host}` : `${s.peer_host} · pairs on first connect`];
  }
}

function sharingNote(s: Status): string {
  if (errors.mouse) return errors.mouse;
  if (s.sharing_paused) return "Sharing is paused.";
  if (!s.mouse_sharing) return "";
  if (s.mouse_state === "failed") return `Mouse and keyboard aren't working: ${s.mouse_reason ?? "unknown reason"}.`;
  if (s.mouse_state === "running") {
    return `Push the pointer off the ${s.mac_side === "top" ? "top" : s.mac_side === "bottom" ? "bottom" : s.mac_side} edge to reach the MacBook. Ctrl+Shift+Alt+Win takes it back.`;
  }
  return s.paired ? "Starts in a moment." : "Starts once this PC and the Mac are paired.";
}

function renderClient(s: Status) {
  const [title, detail] = heroText(s);
  $("hero-title").textContent = title;
  $("hero-detail").textContent = detail;
  $("dot").dataset.state = s.state;
  $("subtitle").textContent = "An extra display and a shared mouse for your Mac";

  const toggle = $<HTMLButtonElement>("toggle");
  toggle.textContent = idle(s) ? "Connect" : s.state === "connecting" ? "Cancel" : "Disconnect";
  toggle.dataset.active = String(!idle(s));
  toggle.disabled = !s.peer_host || !!s.config_error;
  const failure = $("failure");
  const failureText = s.state === "failed" ? `${s.part ?? "Something"} failed: ${s.reason ?? "unknown reason"}` : "";
  failure.textContent = errors.toggle || failureText;
  failure.hidden = !failure.textContent;

  // Sharing
  $<HTMLInputElement>("sharing-on").checked = !s.sharing_paused;
  const mouse = $<HTMLInputElement>("mouse-on");
  mouse.checked = s.mouse_sharing;
  mouse.disabled = s.sharing_paused;
  if (s.clipboard_sharing !== undefined) {
    $("clipboard-row").hidden = false;
    const clip = $<HTMLInputElement>("clipboard-on");
    clip.checked = s.clipboard_sharing;
    clip.disabled = s.sharing_paused;
  }
  if (s.language_follow !== undefined) {
    $("language-row").hidden = false;
    const lang = $<HTMLInputElement>("language-on");
    lang.checked = s.language_follow;
    lang.disabled = s.sharing_paused || !s.mouse_sharing;
  }
  $("mouse-status").textContent = sharingNote(s);

  // Display
  $<HTMLFieldSetElement>("settings").disabled = !idle(s);
  setSelect($<HTMLSelectElement>("resolution"), `${s.settings.width}x${s.settings.height}`, `${s.settings.width} × ${s.settings.height}`);
  setSelect($<HTMLSelectElement>("fps"), String(s.settings.fps), `${s.settings.fps} fps`);
  setSelect($<HTMLSelectElement>("bitrate"), String(s.settings.bitrate_mbps), `${s.settings.bitrate_mbps} Mbps`);
  for (const b of $("mode").querySelectorAll("button")) {
    b.setAttribute("aria-pressed", String(b.dataset.value === s.settings.window_mode));
  }
  $("settings-hint").textContent = errors.settings || (idle(s) ? "" : "Disconnect to change the display.");

  // Mac
  const host = $<HTMLInputElement>("host");
  if (document.activeElement !== host) host.value = s.peer_host ?? "";
  host.disabled = !idle(s);
  $("pair-label").textContent = s.paired ? `Paired with ${macName(s)}` : "Not paired yet";
  const others = (s.peer_addresses ?? []).map((a) => `${a} (${linkKind(a)})`);
  $("pair-sub").textContent = s.paired
    ? others.length
      ? `Also reachable at ${others.join(", ")}. The fastest one is used.`
      : "Connects without asking."
    : "The Mac asks you to allow this PC the first time.";
  $("forget").hidden = !s.paired;
  $<HTMLButtonElement>("forget").disabled = !idle(s);
  $("peer-hint").textContent = errors.peer || notes.peer;

  // App
  $<HTMLInputElement>("tray-on").checked = s.close_to_tray;

  drawArrangement(s);
}

/** Puts `value` in a select, adding it as an option when it is not one of the presets. */
function setSelect(select: HTMLSelectElement, value: string, label: string) {
  if (![...select.options].some((o) => o.value === value)) select.add(new Option(label, value));
  select.value = value;
}

function renderSystem() {
  if (!system || !current) return;
  $("missing").hidden = !system.missing;
  $("missing-text").textContent = system.missing
    ? `${system.missing} isn't installed. Sidescreen needs it to show the display.`
    : "";
  for (const id of ["login-on", "host-login-on"]) $<HTMLInputElement>(id).checked = system.start_at_login;
  if (current.role === "host" || location.hash === "#host") {
    $("step-screen").dataset.done = String(system.screen_recording === true);
    $("step-access").dataset.done = String(system.accessibility === true);
    $("step-engine").dataset.done = String(!system.missing);
    $("engine-sub").textContent = system.missing ? "Sunshine sends the display to your PC." : "Sunshine is installed.";
  }
}

// ---------- arrangement editor ----------

const MAC_SCALE = 0.62; // the MacBook drawn a bit smaller than a monitor

function drawArrangement(s: Status) {
  const box = $("arrangement");
  if (monitors.length === 0) {
    box.innerHTML = "";
    return;
  }
  const minX = Math.min(...monitors.map((m) => m.left));
  const minY = Math.min(...monitors.map((m) => m.top));
  const maxX = Math.max(...monitors.map((m) => m.right));
  const maxY = Math.max(...monitors.map((m) => m.bottom));
  const avgW = monitors.reduce((a, m) => a + (m.right - m.left), 0) / monitors.length;
  const avgH = monitors.reduce((a, m) => a + (m.bottom - m.top), 0) / monitors.length;
  const macW = avgW * MAC_SCALE;
  const macH = avgH * MAC_SCALE;
  const gap = avgW * 0.04;
  // Room for the MacBook on every side, so it can be dragged anywhere.
  const worldL = minX - macW - gap, worldR = maxX + macW + gap;
  const worldT = minY - macH - gap, worldB = maxY + macH + gap;
  const scale = Math.min((box.clientWidth - 8) / (worldR - worldL), (box.clientHeight - 8) / (worldB - worldT));
  const offX = (box.clientWidth - (worldR - worldL) * scale) / 2;
  const offY = (box.clientHeight - (worldB - worldT) * scale) / 2;
  const px = (x: number) => offX + (x - worldL) * scale;
  const py = (y: number) => offY + (y - worldT) * scale;

  const viewer = monitorToward(s.mac_side);
  box.innerHTML = "";
  monitors.forEach((m, i) => {
    const el = document.createElement("div");
    el.className = "screen" + (m === viewer ? " viewer" : "");
    Object.assign(el.style, {
      left: `${px(m.left) + 1}px`,
      top: `${py(m.top) + 1}px`,
      width: `${(m.right - m.left) * scale - 2}px`,
      height: `${(m.bottom - m.top) * scale - 2}px`,
    });
    el.textContent = m === viewer ? "Mac display opens here" : `Screen ${i + 1}`;
    box.appendChild(el);
  });

  const cx = (minX + maxX) / 2, cy = (minY + maxY) / 2;
  const spot: Record<Side, [number, number]> = {
    right: [maxX + gap, cy - macH / 2],
    left: [minX - gap - macW, cy - macH / 2],
    top: [cx - macW / 2, minY - gap - macH],
    bottom: [cx - macW / 2, maxY + gap],
  };
  const mac = document.createElement("div");
  mac.className = "screen mac";
  mac.textContent = "MacBook";
  const place = (side: Side) => {
    const [x, y] = spot[side];
    Object.assign(mac.style, { left: `${px(x)}px`, top: `${py(y)}px`, width: `${macW * scale}px`, height: `${macH * scale}px` });
  };
  place(s.mac_side);
  box.appendChild(mac);

  // Drag anywhere; on release it snaps to the nearest side.
  mac.onpointerdown = (down) => {
    if (!current) return;
    mac.setPointerCapture(down.pointerId);
    mac.classList.add("dragging");
    const startL = mac.offsetLeft, startT = mac.offsetTop;
    mac.onpointermove = (move) => {
      mac.style.left = `${startL + move.clientX - down.clientX}px`;
      mac.style.top = `${startT + move.clientY - down.clientY}px`;
    };
    mac.onpointerup = () => {
      mac.onpointermove = null;
      mac.onpointerup = null;
      mac.classList.remove("dragging");
      const mx = mac.offsetLeft + mac.offsetWidth / 2, my = mac.offsetTop + mac.offsetHeight / 2;
      const nx = (mx - (px(minX) + px(maxX)) / 2) / ((px(maxX) - px(minX)) / 2 || 1);
      const ny = (my - (py(minY) + py(maxY)) / 2) / ((py(maxY) - py(minY)) / 2 || 1);
      const side: Side = Math.abs(nx) >= Math.abs(ny) ? (nx >= 0 ? "right" : "left") : ny >= 0 ? "bottom" : "top";
      place(side);
      if (side !== current?.mac_side) void run("mouse", "set_mac_side", { side });
    };
  };
}

/** The monitor the viewer opens on: the one furthest toward the Mac. */
function monitorToward(side: Side): Monitor | undefined {
  const reach = (m: Monitor) => (side === "right" ? m.right : side === "left" ? -m.left : side === "bottom" ? m.bottom : -m.top);
  return monitors.reduce<Monitor | undefined>((best, m) => (!best || reach(m) > reach(best) ? m : best), undefined);
}

// ---------- commands ----------

async function refresh() {
  current = await invoke<Status>("get_status");
  render(current);
  renderSystem();
}

async function refreshSystem() {
  try {
    system = await invoke<System>("get_system");
  } catch {
    return;
  }
  renderSystem();
}

/** Runs a command, remembering its error (or clearing the last one) for `area`. */
async function run(area: Area | null, command: string, args?: Record<string, unknown>) {
  try {
    await invoke(command, args);
    if (area) errors[area] = "";
  } catch (e) {
    if (area) errors[area] = String(e);
  }
  await refresh();
}

async function saveSettings(windowMode?: string) {
  if (!current) return;
  const [width, height] = $<HTMLSelectElement>("resolution").value.split("x").map(Number);
  const settings = {
    width,
    height,
    fps: Number($<HTMLSelectElement>("fps").value),
    bitrate_mbps: Number($<HTMLSelectElement>("bitrate").value),
    window_mode: windowMode ?? current.settings.window_mode,
  };
  await run("settings", "save_settings", { settings });
}

window.addEventListener("DOMContentLoaded", async () => {
  await listen<Status>("state-changed", (event) => {
    current = event.payload;
    render(current);
    renderSystem();
  });

  $("toggle").addEventListener("click", () => {
    if (!current) return;
    void run("toggle", idle(current) ? "connect" : "disconnect");
  });

  $("sharing-on").addEventListener("change", (e) => {
    void run("mouse", "set_sharing_paused", { paused: !(e.target as HTMLInputElement).checked });
  });
  $("mouse-on").addEventListener("change", (e) => {
    void run("mouse", "set_mouse_sharing", { enabled: (e.target as HTMLInputElement).checked });
  });
  $("clipboard-on").addEventListener("change", (e) => {
    void run("mouse", "set_clipboard_sharing", { enabled: (e.target as HTMLInputElement).checked });
  });
  $("language-on").addEventListener("change", (e) => {
    void run("mouse", "set_language_follow", { enabled: (e.target as HTMLInputElement).checked });
  });

  for (const id of ["resolution", "fps", "bitrate"]) $(id).addEventListener("change", () => void saveSettings());
  for (const b of $("mode").querySelectorAll<HTMLButtonElement>("button")) {
    b.addEventListener("click", () => void saveSettings(b.dataset.value));
  }

  $("peer-setup").addEventListener("submit", (event) => {
    event.preventDefault();
    notes.peer = "";
    void run("peer", "set_peer_host", { host: $<HTMLInputElement>("host").value.trim() });
  });
  $("find").addEventListener("click", async () => {
    errors.peer = "";
    notes.peer = "Looking…";
    if (current) render(current);
    const found = await invoke<{ name: string; host: string } | null>("find_mac");
    if (found) {
      $<HTMLInputElement>("host").value = found.host;
      notes.peer = `Found ${found.name}. Press Save to use it.`;
    } else {
      notes.peer = "No Mac found. Open Sidescreen on the Mac, or type its address.";
    }
    if (current) render(current);
  });
  $("forget").addEventListener("click", () => void run("peer", "forget_peer"));
  $("reset-config").addEventListener("click", () => void run(null, "reset_config"));

  for (const id of ["login-on", "host-login-on"]) {
    $(id).addEventListener("change", async (event) => {
      try {
        await invoke("set_start_at_login", { enabled: (event.target as HTMLInputElement).checked });
      } finally {
        await refreshSystem();
      }
    });
  }
  $("tray-on").addEventListener("change", (e) => {
    void run(null, "set_close_to_tray", { enabled: (e.target as HTMLInputElement).checked });
  });

  const install = async () => {
    try {
      await invoke("install_missing");
      $("missing-text").textContent = "Installing… follow the window that opened.";
      $("engine-sub").textContent = "Installing… follow the Terminal window.";
    } catch (e) {
      $("missing-text").textContent = String(e);
    }
  };
  $("install-missing").addEventListener("click", install);
  $("install-engine").addEventListener("click", install);
  $("perm-screen").addEventListener("click", () => void run(null, "open_privacy_settings", { kind: "screen" }));
  $("perm-access").addEventListener("click", () => void run(null, "open_privacy_settings", { kind: "accessibility" }));
  $("done").addEventListener("click", () => void invoke("close_window"));

  try {
    monitors = await invoke<Monitor[]>("get_monitors");
  } catch {
    monitors = [];
  }
  window.addEventListener("resize", () => current && drawArrangement(current));

  await refresh();
  await refreshSystem();
  window.setInterval(() => void refreshSystem(), 2000);
});

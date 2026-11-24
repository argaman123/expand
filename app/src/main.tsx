import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { useStatus } from "./api";
import { PcApp } from "./pc/PcApp";
import { MacSetup } from "./mac/MacSetup";

// Before the status says which computer this is: the Mac's web view says Macintosh.
const LOOKS_LIKE_MAC = /Macintosh|Mac OS X/.test(navigator.userAgent);

function App() {
  const status = useStatus();
  // "#host" in the address shows the Mac's setup guide anywhere (for previews).
  const host = location.hash === "#host" || (status ? status.role === "host" : LOOKS_LIKE_MAC);
  if (host) return status ? <MacSetup status={status} /> : null;
  // The PC window has no system frame: its own title bar shows at once, the rest with the status.
  return <PcApp status={status} />;
}

// In a plain browser while developing, a stand-in for the Tauri runtime (never in a build).
const runtime = import.meta.env.DEV && !("__TAURI_INTERNALS__" in window) ? import("./dev/mockTauri") : Promise.resolve();
void runtime.then(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  ),
);

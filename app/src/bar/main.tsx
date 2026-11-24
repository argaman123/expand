import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { FluentProvider } from "@fluentui/react-components";
import { useStatus } from "../api";
import { THEME } from "../pc/theme";
import { NotchBar } from "./NotchBar";
import "../brand.css";
import "./bar.css";

/** The bar's page: the growing notch over the full-screen display. Where the bar sits, how big
 *  its window is, and when it shows are the app's (bar.rs). */
function BarPage() {
  const status = useStatus();
  return (
    <FluentProvider theme={THEME} className="ss-barpage">
      <NotchBar status={status} />
    </FluentProvider>
  );
}

// In a plain browser while developing, a stand-in for the Tauri runtime (never in a build).
const runtime = import.meta.env.DEV && !("__TAURI_INTERNALS__" in window) ? import("../dev/mockTauri") : Promise.resolve();
void runtime.then(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <BarPage />
    </StrictMode>,
  ),
);

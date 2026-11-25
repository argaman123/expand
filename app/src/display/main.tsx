import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { FluentProvider } from "@fluentui/react-components";
import { useStatus } from "../api";
import { THEME } from "../pc/theme";
import { DisplayTitleBar } from "./DisplayTitleBar";
import { Placeholder } from "./Placeholder";
import "../brand.css";
import "./display.css";

/** The display window's page: its title bar, and what shows where the Mac's display goes until it
 *  is there. The Mac's display sits below the title bar (or over all of the page when full
 *  screen), as a window of its own laid on top of this page. */
function DisplayPage() {
  const status = useStatus();
  return (
    <FluentProvider theme={THEME} className="ex-display">
      <DisplayTitleBar status={status} />
      <Placeholder status={status} />
    </FluentProvider>
  );
}

// In a plain browser while developing, a stand-in for the Tauri runtime (never in a build).
const runtime = import.meta.env.DEV && !("__TAURI_INTERNALS__" in window) ? import("../dev/mockTauri") : Promise.resolve();
void runtime.then(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <DisplayPage />
    </StrictMode>,
  ),
);

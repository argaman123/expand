import { useEffect, useState } from "react";
import { getMonitors, type Monitor } from "../api";

export function useMonitors(): Monitor[] {
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  useEffect(() => {
    let alive = true;
    let last = "";
    const load = () =>
      getMonitors()
        .then((m) => m, () => [] as Monitor[]) // a failed query shows an empty stage; the next poll retries
        .then((m) => {
          const key = JSON.stringify(m);
          if (alive && key !== last) {
            last = key;
            setMonitors(m);
          }
        });
    load();
    const t = window.setInterval(load, 5000);
    window.addEventListener("focus", load);
    return () => {
      alive = false;
      window.clearInterval(t);
      window.removeEventListener("focus", load);
    };
  }, []);
  return monitors;
}

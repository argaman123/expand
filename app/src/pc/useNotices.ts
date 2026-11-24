import { useEffect, useRef } from "react";
import { connectMac, displayStart, type Status } from "../api";
import { look } from "./Header";

export type Notice = {
  title: string;
  body?: string;
  intent: "success" | "error";
  /** A newer notice with the same slot replaces the older one on screen. */
  slot?: string;
  /** Label and handler of the toast's action. */
  action?: { label: string; run: () => void };
};

const DEDUPE_MS = 2000;
const BLIP_MS = 5000;
const FAILING = new Set(["busy", "blocked", "ended_by_mac", "version"]);

/** The toasts a status change calls for; `prev` is the status before it. */
export function noticesFor(prev: Status, next: Status): Notice[] {
  const out: Notice[] = [];
  if (prev.connection !== next.connection) {
    if (next.connection === "connected") {
      const where = next.peer_host ?? "";
      const body = next.latency_ms != null ? `${where} · ${next.latency_ms} ms` : where;
      out.push({ title: "Connected", body: body || undefined, intent: "success" });
    } else if (FAILING.has(next.connection)) {
      const key = next.peers.find((p) => p.current)?.key;
      out.push({
        title: look(next).text,
        intent: "error",
        action: { label: "Retry", run: () => void connectMac(key).catch(() => {}) },
      });
    }
  }
  if (prev.state !== next.state) {
    if (next.state === "connected") {
      const s = next.settings;
      const monitor = next.viewer_monitor ?? s.monitor; // as resolved: the chosen one may be unplugged
      const parts = [monitor != null ? `Monitor ${monitor}` : null, `${s.width} × ${s.height}`, `${s.fps} fps`];
      out.push({ title: "Extended display", body: parts.filter(Boolean).join(" · "), intent: "success", slot: "display" });
    } else if (next.state === "failed") {
      // The display needs the link: while it is down the action is Connect, and a Retry that
      // finds it down meanwhile connects rather than failing unseen.
      const key = next.peers.find((p) => p.current)?.key;
      const connect = () => void connectMac(key).catch(() => {});
      const retry = () =>
        void displayStart().catch((e) => {
          if (/connect to the Mac first/i.test(String(e))) connect();
        });
      out.push({
        title: next.part ?? "Extended display",
        body: next.reason ?? undefined,
        intent: "error",
        action: next.connection === "connected" ? { label: "Retry", run: retry } : { label: "Connect", run: connect },
      });
    }
  }
  return out;
}

/** Calls `notify` for status transitions only; the first status after mount is the baseline and never toasts. */
export function useNotices(status: Status, notify: (n: Notice) => void) {
  const prev = useRef<Status | null>(null);
  const seen = useRef(new Map<string, number>());
  const lastConnected = useRef(0); // when the connection was last seen connected
  const send = useRef(notify);
  send.current = notify;
  useEffect(() => {
    const before = prev.current;
    prev.current = status;
    const now = Date.now();
    const sinceConnected = now - lastConnected.current; // measured before this status counts
    if (before?.connection === "connected" || status.connection === "connected") lastConnected.current = now;
    if (!before) return;
    for (const n of noticesFor(before, status)) {
      const key = `${n.title}|${n.body ?? ""}`;
      const at = seen.current.get(key);
      if (at !== undefined && now - at < DEDUPE_MS) continue;
      if (n.title === "Connected" && sinceConnected < BLIP_MS) continue; // a blip, not a new connection
      seen.current.set(key, now);
      send.current(n);
    }
  }, [status]);
}

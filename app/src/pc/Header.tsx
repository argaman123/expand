import { useEffect, useRef, useState } from "react";
import {
  Button, Input, Menu, MenuDivider, MenuGroupHeader, MenuItem, MenuItemRadio, MenuList, MenuPopover, MenuTrigger,
  PresenceBadge, Spinner, Tooltip,
} from "@fluentui/react-components";
import {
  Add20Regular, ArrowSync16Regular, ChevronDown20Regular, Delete16Regular, Edit16Regular, Laptop20Regular, Search20Regular,
  Settings24Regular,
} from "@fluentui/react-icons";
import { connectMac, disconnectMac, forgetMac, reconnectMac, setMacAddress, type Status } from "../api";
import { PairDialog, type PairTarget } from "./PairDialog";
import { ErrorLine, failureText, type Failure } from "./ErrorLine";

type Look = { text: string; badge: "available" | "busy" | "offline" | "spinner" };

export function look(status: Status): Look {
  // The device's name is the title right above: the line never repeats it (a long one would crowd out the address).
  switch (status.connection) {
    case "connected": return { text: "Connected", badge: "available" };
    case "connecting":
    case "lost": return { text: "Connecting", badge: "spinner" };
    case "ended_by_mac": return { text: "Disconnected by the device", badge: "offline" };
    case "busy": return { text: `In use · ${status.busy_by ?? "another PC"}`, badge: "busy" };
    case "blocked": return { text: "Blocked", badge: "offline" };
    case "unknown": return { text: "Not paired", badge: "offline" };
    case "version": return { text: "Update the device", badge: "offline" };
    default: return { text: "Not connected", badge: "offline" };
  }
}


export function Header({ status, onOpenSettings }: { status: Status; onOpenSettings: () => void }) {
  const current = status.peers.find((p) => p.current);
  const key = current?.key;
  const host = current?.host ?? status.peer_host ?? "";
  const name = current?.name ?? status.peer_name ?? "No device";
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(host);
  const [editError, setEditError] = useState<Failure | null>(null);
  const [pairing, setPairing] = useState<PairTarget | null>(null);
  const [actionError, setActionError] = useState<Failure | null>(null);
  const [tipOpen, setTipOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [forgetting, setForgetting] = useState<string | null>(null); // the device whose Forget asks to be confirmed
  const bins = useRef(new Map<string, HTMLElement>()); // each device's Forget item

  useEffect(() => {
    setEditing(false);
    setEditError(null);
    setActionError(null);
  }, [key]);

  useEffect(() => {
    if (status.connection === "connected") setActionError(null);
  }, [status.connection]);

  // Runs an action; when it fails, its error shows with a Retry that runs it again.
  const act = (run: () => Promise<unknown>) => {
    setActionError(null);
    run().catch((e) => setActionError({ text: failureText(e), retry: () => act(run) }));
  };
  // A device's Forget item: the first press asks ("Forget <name>?"), the second forgets and closes
  // the menu. Escape, or leaving the item, asks no more.
  const pressForget = (k: string) => {
    if (forgetting !== k) return setForgetting(k);
    setForgetting(null);
    setMenuOpen(false);
    act(() => forgetMac(k));
  };
  const cancelEdit = () => {
    setEditing(false);
    setEditError(null);
  };

  const { text, badge } = look(status);
  const connected = status.connection === "connected";
  const busyTrying = status.connection === "connecting" || status.connection === "lost";
  const failure = editError ?? actionError;
  // The round trip changes every second: it shows on hover only, never in the line itself.
  const roundTrip = connected && status.latency_ms != null ? `Round trip · ${status.latency_ms} ms` : null;

  const connectTo = (k: string) => {
    const peer = status.peers.find((p) => p.key === k);
    if (!peer) return;
    if (!peer.paired) {
      setPairing({ mode: "peer", peer });
      return;
    }
    act(() => connectMac(k));
  };

  const connect = () => {
    if (!current) return setPairing({ mode: "find" });
    if (status.connection === "unknown" || !current.paired) return setPairing({ mode: "peer", peer: current });
    connectTo(current.key);
  };

  // Confirming the address saves it, then connects to the Mac there (a Mac not paired yet asks
  // for its code).
  const save = async () => {
    if (!key || !current) return;
    const typed = draft.trim();
    try {
      const now = await setMacAddress(key, typed);
      setEditing(false);
      setEditError(null);
      if (status.connection === "unknown" || !current.paired) setPairing({ mode: "peer", peer: { ...current, key: now, host: typed } });
      else act(() => connectMac(now));
    } catch (e) {
      setEditError({ text: failureText(e), retry: () => void save() });
    }
  };

  return (
    <div className="ss-header">
      <div className="ss-who">
        <Menu
          checkedValues={{ mac: key ? [key] : [] }}
          onCheckedValueChange={(_, d) => d.checkedItems[0] && connectTo(d.checkedItems[0])}
          open={menuOpen}
          onOpenChange={(_, d) => {
            setMenuOpen(d.open);
            setForgetting(null);
          }}
        >
          <MenuTrigger disableButtonEnhancement>
            <Button appearance="transparent" className="ss-macname" iconPosition="after" icon={<ChevronDown20Regular />}>
              <span className="ss-macname-text">{name}</span>
            </Button>
          </MenuTrigger>
          <MenuPopover>
            <MenuList>
              {status.peers.length > 0 && <MenuGroupHeader>Devices</MenuGroupHeader>}
              {status.peers.map((p) => {
                const asking = forgetting === p.key;
                return (
                  // A row: the device (picking it connects), then its own Forget item, which arrow
                  // keys reach next and the Delete key jumps to.
                  <div key={p.key} role="presentation" className="ss-dev-row">
                    <MenuItemRadio
                      name="mac"
                      value={p.key}
                      icon={<Laptop20Regular />}
                      secondaryContent={p.host}
                      aria-keyshortcuts="Delete"
                      className="ss-dev"
                      onKeyDown={(e) => {
                        if (e.key !== "Delete") return;
                        e.preventDefault();
                        setForgetting(p.key);
                        bins.current.get(p.key)?.focus();
                      }}
                    >
                      {p.name}
                    </MenuItemRadio>
                    <MenuItem
                      ref={(el) => {
                        if (el) bins.current.set(p.key, el);
                        else bins.current.delete(p.key);
                      }}
                      persistOnClick
                      className={`ss-forget${asking ? " asking" : ""}`}
                      aria-label={asking ? `Forget ${p.name}?` : `Forget ${p.name}`}
                      icon={asking ? undefined : <Delete16Regular />}
                      onClick={() => pressForget(p.key)}
                      onBlur={() => asking && setForgetting(null)}
                      onKeyDown={(e) => {
                        if (e.key !== "Escape" || !asking) return;
                        e.preventDefault();
                        e.stopPropagation(); // asks no more; the menu stays
                        setForgetting(null);
                      }}
                    >
                      {asking ? "Forget" : null}
                    </MenuItem>
                  </div>
                );
              })}
              {status.peers.length > 0 && <MenuDivider />}
              <MenuItem icon={<Search20Regular />} onClick={() => setPairing({ mode: "find" })}>Find devices</MenuItem>
              <MenuItem icon={<Add20Regular />} onClick={() => setPairing({ mode: "address" })}>Add by address</MenuItem>
            </MenuList>
          </MenuPopover>
        </Menu>
        <div className="ss-line">
          <Tooltip
            content={roundTrip ?? ""}
            relationship="description"
            positioning="below"
            withArrow
            visible={tipOpen && roundTrip !== null}
            onVisibleChange={(_, d) => setTipOpen(d.visible)}
          >
            <span className="ss-status">
              <span className="ss-status-icon">
                {badge === "spinner" ? <Spinner size="extra-tiny" /> : <PresenceBadge status={badge} size="small" />}
              </span>
              {text}
            </span>
          </Tooltip>
          {key &&
            (editing ? (
              <span className="ss-edit">
                <Input
                  size="medium"
                  value={draft}
                  onChange={(_, d) => setDraft(d.value)}
                  autoFocus
                  placeholder="192.168.1.20"
                  aria-label="Device address"
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void save();
                    if (e.key === "Escape") cancelEdit();
                  }}
                />
                <Button size="small" appearance="primary" onClick={() => void save()}>Save</Button>
                <Button size="small" appearance="subtle" onClick={cancelEdit}>Cancel</Button>
              </span>
            ) : (
              <Tooltip content="Edit address" relationship="label" withArrow>
                <Button
                  appearance="subtle"
                  size="small"
                  className="ss-addr"
                  icon={<Edit16Regular />}
                  iconPosition="after"
                  onClick={() => {
                    setDraft(host);
                    setEditError(null);
                    setEditing(true);
                  }}
                >
                  {host}
                </Button>
              </Tooltip>
            ))}
          {connected ? (
            <>
              <Button appearance="subtle" size="small" onClick={() => act(disconnectMac)}>Disconnect</Button>
              <Tooltip content="Reconnect" relationship="label" withArrow>
                <Button appearance="subtle" size="small" icon={<ArrowSync16Regular />} aria-label="Reconnect" onClick={() => act(reconnectMac)} />
              </Tooltip>
            </>
          ) : busyTrying ? (
            <Button appearance="subtle" size="small" onClick={() => act(disconnectMac)}>Cancel</Button>
          ) : (
            <Button appearance="subtle" size="small" onClick={connect}>Connect</Button>
          )}
          {failure && <ErrorLine className="ss-header-err" failure={failure} />}
        </div>
      </div>
      <Tooltip content="Settings" relationship="label" withArrow>
        <Button appearance="subtle" size="large" icon={<Settings24Regular />} aria-label="Settings" onClick={onOpenSettings} className="ss-gear" />
      </Tooltip>
      <PairDialog target={pairing} peers={status.peers} onClose={() => setPairing(null)} />
    </div>
  );
}

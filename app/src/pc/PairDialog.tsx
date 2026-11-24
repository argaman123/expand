import { useEffect, useRef, useState } from "react";
import {
  Badge, Button, Dialog, DialogActions, DialogBody, DialogContent, DialogSurface, DialogTitle, Field, Input,
  Spinner, Text,
} from "@fluentui/react-components";
import { Laptop20Regular, Search20Regular } from "@fluentui/react-icons";
import { addMacByAddress, connectMac, findMacs, type Found, type Peer } from "../api";
import { knownPeer, onePerMac } from "./found";

/** What the dialog is for: find devices on the network, type an address, or pair a saved one. */
export type PairTarget =
  | { mode: "find" }
  | { mode: "address" }
  | { mode: "peer"; peer: Peer };

type Pick = { name: string; host: string; key?: string };

/** The backend says "pairing was declined" for a wrong code. */
function explain(e: unknown): string {
  const text = String(e instanceof Error ? e.message : e);
  return /declined|rejected/i.test(text) ? "Wrong code" : text;
}

export function PairDialog({ target, peers, onClose }: { target: PairTarget | null; peers: Peer[]; onClose: () => void }) {
  const [found, setFound] = useState<Found[] | null>(null);
  const [picked, setPicked] = useState<Pick | null>(null);
  const [host, setHost] = useState("");
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const round = useRef(0);

  const search = () => {
    const mine = ++round.current;
    setFound(null);
    setError(null);
    findMacs()
      .then((list) => mine === round.current && setFound(onePerMac(list)))
      .catch((e) => {
        if (mine !== round.current) return;
        setFound([]);
        setError(explain(e));
      });
  };

  const back = () => {
    setPicked(null);
    setCode("");
    setError(null);
  };

  useEffect(() => {
    round.current++; // a result still on its way belongs to the old dialog
    setPicked(null);
    setHost("");
    setCode("");
    setError(null);
    setBusy(false);
    setFound(null);
    if (!target) return;
    if (target.mode === "peer") setPicked({ name: target.peer.name, host: target.peer.host, key: target.peer.key });
    if (target.mode === "find") search();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target]);

  const choose = async (f: Found) => {
    if (busy) return;
    const known = knownPeer(peers, f);
    setError(null);
    // A Mac already in the list: switch to it. One that is not paired yet asks for its code.
    if (known?.paired) {
      setBusy(true);
      try {
        await connectMac(known.key);
        onClose();
      } catch (e) {
        setError(explain(e));
      } finally {
        setBusy(false);
      }
      return;
    }
    setPicked({ name: known?.name ?? f.name, host: f.host, key: known?.key });
  };

  const entering = picked !== null || target?.mode === "address";
  const address = picked ? picked.host : host.trim();
  const ready = code.length === 6 && address !== "" && !busy;

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      if (picked?.key) await connectMac(picked.key, code);
      else await addMacByAddress(address, code);
      onClose();
    } catch (e) {
      setError(explain(e));
    } finally {
      setBusy(false);
    }
  };

  const title = picked ? `Pair with ${picked.name}` : target?.mode === "address" ? "Add by address" : "Find devices";

  return (
    <Dialog open={target !== null} onOpenChange={(_, d) => !d.open && onClose()}>
      <DialogSurface className="ss-dialog">
        <DialogBody>
          <DialogTitle>{title}</DialogTitle>
          <DialogContent>
            {!entering && (
              <div className="ss-found">
                {found === null && <Spinner size="small" label="Looking for devices" labelPosition="after" />}
                {found?.length === 0 && <Text>No devices found</Text>}
                {found?.map((f) => {
                  const added = knownPeer(peers, f) !== undefined;
                  return (
                    <button type="button" key={`${f.id}${f.host}`} className="ss-found-row" disabled={busy} onClick={() => void choose(f)}>
                      <Laptop20Regular />
                      <span className="ss-found-name">{f.name}</span>
                      <span className="ss-found-host">{f.host}</span>
                      {f.pairing && <Badge appearance="tint" color="brand" size="small">Pairing</Badge>}
                      {added && <span className="ss-found-added">Added</span>}
                    </button>
                  );
                })}
                {error && <Text className="ss-err">{error}</Text>}
              </div>
            )}
            {entering && (
              <div className="ss-pair-form">
                {picked ? (
                  <Text>{picked.host}</Text>
                ) : (
                  <Field label="Address">
                    <Input value={host} onChange={(_, d) => setHost(d.value)} placeholder="192.168.1.20" autoFocus />
                  </Field>
                )}
                <Field label={`Code shown on ${picked?.name ?? "the device"}`} validationMessage={error ?? undefined} validationState={error ? "error" : "none"}>
                  <Input
                    className="ss-code"
                    value={code}
                    inputMode="numeric"
                    maxLength={6}
                    autoFocus={picked !== null}
                    placeholder="000000"
                    onChange={(_, d) => setCode(d.value.replace(/\D/g, "").slice(0, 6))}
                    onKeyDown={(e) => e.key === "Enter" && void submit()}
                  />
                </Field>
              </div>
            )}
          </DialogContent>
          <DialogActions>
            {!entering && (
              <Button icon={<Search20Regular />} onClick={search} disabled={found === null}>Search again</Button>
            )}
            {entering && target?.mode === "find" && picked && (
              <Button onClick={back} disabled={busy}>Back</Button>
            )}
            {entering && (
              <Button appearance="primary" onClick={() => void submit()} disabled={!ready}>
                {busy ? <Spinner size="tiny" /> : "Connect"}
              </Button>
            )}
            <Button appearance="secondary" onClick={onClose}>Cancel</Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

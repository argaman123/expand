import type { Found, Peer } from "../api";

/** One entry per Mac (by id): the address that is in pairing mode wins. */
export function onePerMac(list: Found[]): Found[] {
  const out = new Map<string, Found>();
  for (const f of list) {
    const k = f.id !== "" ? f.id : f.host;
    const seen = out.get(k);
    if (!seen || (f.pairing && !seen.pairing)) out.set(k, f);
  }
  return [...out.values()];
}

/** A saved Mac's id, or "" while it is known by address alone (its key is then `host:…`). */
const peerId = (p: Peer) => (p.key.startsWith("host:") ? "" : p.key);

/** The Mac in the list that a found Mac is: by id when both ids are known, else by address. */
export function knownPeer(peers: Peer[], f: Found): Peer | undefined {
  return peers.find((p) => {
    const id = peerId(p);
    if (id !== "" && f.id !== "") return id === f.id;
    return p.host === f.host || p.addresses.includes(f.host);
  });
}

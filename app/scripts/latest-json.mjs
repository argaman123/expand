// Writes latest.json, the manifest the PC app's updater reads from
// https://github.com/argaman123/expand/releases/latest/download/latest.json
// It names the signed installer of the tagged release and carries its signature (the .sig the
// build writes next to the installer when it has the signing key).
//
// Usage: node scripts/latest-json.mjs <version> <installer.exe.sig> <out.json> [notes]
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const RELEASES = "https://github.com/argaman123/expand/releases/download";

/** The file name the Windows bundle gives the installer, which is also the release asset's name. */
export const installerName = (version) => `Expand_${version}_x64-setup.exe`;

/** The manifest for `version`, whose installer was signed with `signature`. */
export function latestJson({ version, signature, notes = "", pubDate = new Date() }) {
  const v = String(version).trim().replace(/^v/, "");
  if (!/^\d+\.\d+\.\d+$/.test(v)) throw new Error(`not a release version: ${version}`);
  const sig = String(signature).trim();
  if (!sig) throw new Error("the signature is empty");
  const platform = { url: `${RELEASES}/v${v}/${installerName(v)}`, signature: sig };
  return {
    version: v,
    notes,
    pub_date: pubDate.toISOString().replace(/\.\d+Z$/, "Z"),
    // The updater looks for `windows-x86_64-nsis` first, then `windows-x86_64`.
    platforms: { "windows-x86_64-nsis": platform, "windows-x86_64": { ...platform } },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [version, sigPath, outPath, notes = ""] = process.argv.slice(2);
  if (!version || !sigPath || !outPath) {
    console.error("usage: node scripts/latest-json.mjs <version> <installer.exe.sig> <out.json> [notes]");
    process.exit(2);
  }
  const manifest = latestJson({ version, signature: readFileSync(sigPath, "utf8"), notes });
  writeFileSync(outPath, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`wrote ${outPath} for ${manifest.version}`);
}

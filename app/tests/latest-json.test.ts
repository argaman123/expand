// The updater's manifest (latest.json) that a release publishes. Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { installerName, latestJson } from "../scripts/latest-json.mjs";

const SIG = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQ==";

test("the installer is named as the Windows bundle names it", () => {
  assert.equal(installerName("1.2.0"), "Sidescreen_1.2.0_x64-setup.exe");
});

test("the manifest points both Windows targets at the tagged release's installer", () => {
  const m = latestJson({ version: "1.2.0", signature: `${SIG}\r\n`, pubDate: new Date("2026-11-24T08:30:15.123Z") });
  const url = "https://github.com/argaman123/sidescreen/releases/download/v1.2.0/Sidescreen_1.2.0_x64-setup.exe";
  assert.deepEqual(m, {
    version: "1.2.0",
    notes: "",
    pub_date: "2026-11-24T08:30:15Z",
    platforms: {
      "windows-x86_64-nsis": { url, signature: SIG },
      "windows-x86_64": { url, signature: SIG },
    },
  });
});

test("a leading v on the version is dropped", () => {
  assert.equal(latestJson({ version: "v1.2.0", signature: SIG }).version, "1.2.0");
});

test("notes are carried", () => {
  assert.equal(latestJson({ version: "1.2.0", signature: SIG, notes: "Fixes" }).notes, "Fixes");
});

test("a version that is not x.y.z is refused", () => {
  assert.throws(() => latestJson({ version: "1.2", signature: SIG }));
  assert.throws(() => latestJson({ version: "latest", signature: SIG }));
});

test("an empty signature is refused", () => {
  assert.throws(() => latestJson({ version: "1.2.0", signature: " \n" }));
});

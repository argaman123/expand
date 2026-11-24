// The brand colours: the app icon's gradient, and the Fluent ramp made from its middle.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { BRAND, BRAND_ON, BRAND_STEPS, BRAND_TEXT, FILL, ICON_STOPS, brandRamp, contrast, luminance } from "../src/brand.ts";

// What is on (tiles, switches): brand.css's --ss-on and the icon or thumb on it, at rest, hovered
// (8% lighter) and pressed (10% darker).
const BRAND_CSS = readFileSync(new URL("../src/brand.css", import.meta.url), "utf8");
const token = (name: string) => BRAND_CSS.match(new RegExp(name + ": *(#[0-9a-fA-F]{6})"))![1];
const darken = (hex: string, t: number) =>
  "#" + [1, 3, 5].map((i) => Math.round(parseInt(hex.slice(i, i + 2), 16) * (1 - t)).toString(16).padStart(2, "0")).join("");

const WINDOW = "#202020"; // the window's backdrop where it is solid

test("the stops are the app icon's gradient (Shore): sky blue into violet", () => {
  assert.deepEqual(BRAND, { start: "#3aa8ff", mid: "#5d8bff", end: "#8a5cf6" });
  for (const file of ["icon.svg", "icon-small.svg"]) {
    const icon = readFileSync(new URL(`../../design/icons/${file}`, import.meta.url), "utf8");
    for (const stop of Object.values(ICON_STOPS)) assert.ok(icon.includes(stop), `${file} has ${stop}`);
  }
  // The icon's ends are pulled apart from the brand's: lighter at the start, deeper at the end.
  assert.ok(luminance(ICON_STOPS.start) > luminance(BRAND.start) && luminance(ICON_STOPS.end) < luminance(BRAND.end));
  for (const [name, stop] of [["--ss-start", BRAND.start], ["--ss-mid", BRAND.mid], ["--ss-end", BRAND.end]]) assert.equal(token(name), stop);
});

test("the ramp has Fluent's 16 steps with the fill at 80", () => {
  const ramp = brandRamp();
  assert.deepEqual(Object.keys(ramp).map(Number), [...BRAND_STEPS]);
  assert.equal(ramp[80], FILL);
  for (const step of BRAND_STEPS) assert.match(ramp[step], /^#[0-9a-f]{6}$/);
});

test("the ramp goes from near black to near white, lighter at every step", () => {
  const ramp = brandRamp();
  const l = BRAND_STEPS.map((s) => luminance(ramp[s]));
  for (let i = 1; i < l.length; i++) assert.ok(l[i] > l[i - 1], `step ${BRAND_STEPS[i]} lighter than ${BRAND_STEPS[i - 1]}`);
  assert.ok(l[0] < 0.01, "step 10 is near black");
  assert.ok(l[l.length - 1] > 0.6, "step 160 is near white");
});

test("text in the brand colour reads on the dark window (step 110 and up)", () => {
  const ramp = brandRamp();
  for (const s of [110, 120, 130, 140]) assert.ok(contrast(ramp[s], WINDOW) >= 4.5, `step ${s}: ${contrast(ramp[s], WINDOW).toFixed(2)}`);
});

test("white reads on the Fluent fill and the step under it", () => {
  assert.ok(contrast("#ffffff", FILL) >= 4.5);
  assert.ok(contrast("#ffffff", brandRamp()[70]) >= 4.5);
});

test("contrast is the WCAG ratio", () => {
  assert.equal(Math.round(contrast("#000000", "#ffffff")), 21);
  assert.equal(contrast("#202020", "#202020"), 1);
});

// Fluent's dark surfaces that brand text can sit on, in each state (webDarkTheme): the window and
// cards at rest; hover greys up to colorNeutralBackground1Hover; the pressed and selected greys.
const SURFACES = {
  rest: ["#202020", "#292929"],
  hover: ["#202020", "#292929", "#333333", "#383838", "#3d3d3d"],
  pressed: ["#141414", "#1f1f1f", "#202020", "#292929", "#2e2e2e"],
  selected: ["#202020", "#292929", "#333333", "#383838"],
} as const;

test("brand text reads in every state on every surface it can sit on", () => {
  const ramp = brandRamp();
  for (const state of Object.keys(SURFACES) as (keyof typeof SURFACES)[]) {
    const color = ramp[BRAND_TEXT[state]];
    for (const bg of SURFACES[state]) {
      assert.ok(contrast(color, bg) >= 4.5, `${state} (step ${BRAND_TEXT[state]}) on ${bg}: ${contrast(color, bg).toFixed(2)}`);
    }
  }
});

test("brand.css: what is on is brand.ts's, and its icon reads at rest, hovered and pressed", () => {
  const on = token("--ss-on");
  const fg = token("--ss-on-fg");
  assert.equal(on, BRAND_ON.fill);
  assert.equal(fg, BRAND_ON.text);
  assert.ok(BRAND_CSS.includes(`--ss-on-fill: linear-gradient(135deg, ${BRAND_ON.from}, ${BRAND_ON.to})`));
  // At rest, hovered (5% darker) and pressed (12% darker): a fill this bright takes no lighter hover
  // under white.
  for (const end of [BRAND_ON.fill, BRAND_ON.from, BRAND_ON.to]) {
    for (const t of [0, 0.05, 0.12]) assert.ok(contrast(fg, darken(end, t)) >= 4.5, `${fg} on ${darken(end, t)}`);
  }
  // Brighter than the screens' blue it matches (round 3: "brighter yet").
  assert.ok(luminance(on) > luminance("#35489d") * 1.6, `${on} is clearly brighter`);
});

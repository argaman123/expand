// The brand, "Shore" (the owner's pick, round 3): the app icon's gradient, a vivid sky blue into a
// vivid violet. Its colours are also in brand.css, which draws the gradient, mac/panel/Brand.swift
// and design/icons. Fluent's theme takes the ramp made from FILL, a deeper blue that white text
// reads on; what is on (tiles, switches) takes BRAND_ON, bright, with a dark icon on it.

export const BRAND = { start: "#3aa8ff", mid: "#5d8bff", end: "#8a5cf6" } as const;
/** Fluent's brand colour: primary buttons, links (its lighter steps), the focus ring. */
export const FILL = "#2f62d6";
/** What is on: the screens' own blue, brighter (`fill`; a tile has the slightest sheen from `from`
 *  to `to`), and the icon on it. Hover dims it a little, pressed more. */
export const BRAND_ON = { fill: "#4f6bdb", from: "#4f6bdb", to: "#4a65d2", text: "#ffffff" } as const;
/** The icon's coloured half: the same gradient with its ends pulled apart (a lighter sky blue, a
 *  deeper violet), so it still reads as a gradient at 32 and 16 px. */
export const ICON_STOPS = { start: "#6fcbff", mid: "#5d8bff", end: "#6f3ae8" } as const;

export const BRAND_STEPS = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160] as const;
export type BrandStep = (typeof BRAND_STEPS)[number];

type Rgb = [number, number, number];

function parse(hex: string): Rgb {
  const n = parseInt(hex.replace("#", "").slice(0, 6), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

const hex = (c: Rgb) => "#" + c.map((x) => Math.round(x).toString(16).padStart(2, "0")).join("");

/** The colour mixed toward `target` (0 = black, 255 = white) by `t` in 0..1. */
function mix(from: Rgb, target: number, t: number): string {
  return hex(from.map((x) => x + (target - x) * t) as Rgb);
}

/** Relative luminance (WCAG). */
export function luminance(color: string): number {
  const [r, g, b] = parse(color).map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The WCAG contrast ratio of two colours. */
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** Fluent's 16 brand steps with the colour at 80, as in Fluent's own ramps: darker below (10 is
 *  79% black), lighter above (160 is 85% white). */
export function brandRamp(color: string = FILL): Record<BrandStep, string> {
  const rgb = parse(color);
  const ramp = {} as Record<BrandStep, string>;
  for (const step of BRAND_STEPS) {
    if (step < 80) ramp[step] = mix(rgb, 0, ((80 - step) / 70) * 0.79);
    else if (step === 80) ramp[step] = hex(rgb);
    else ramp[step] = mix(rgb, 255, ((step - 80) / 80) * 0.85);
  }
  return ramp;
}

/** The ramp step for brand text (links, transparent buttons) in each state, light enough to read
 *  (4.5:1) on every dark surface it can sit on then: the window and cards at rest, Fluent's hover
 *  greys (up to #3d3d3d), its pressed and selected greys. */
export const BRAND_TEXT = { rest: 110, hover: 130, pressed: 120, selected: 120 } as const satisfies Record<string, BrandStep>;

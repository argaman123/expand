export type Res = { w: number; h: number };

const RESOLUTIONS: Res[] = [
  { w: 1920, h: 1080 },
  { w: 2560, h: 1440 },
  { w: 3440, h: 1440 },
  { w: 3840, h: 2160 },
];
const FPS = [30, 60, 120];
const BITRATES = [10, 20, 30, 40, 60, 80, 100];

export const resKey = (r: Res) => `${r.w}x${r.h}`;
export const resLabel = (r: Res) => `${r.w} × ${r.h}`;
export const fpsLabel = (n: number) => `${n} fps`;
export const rateLabel = (n: number) => `${n} Mbps`;

export function resolutionOptions(cur: Res): Res[] {
  return RESOLUTIONS.some((r) => r.w === cur.w && r.h === cur.h)
    ? RESOLUTIONS
    : [...RESOLUTIONS, cur].sort((a, b) => a.w * a.h - b.w * b.h);
}
export const fpsOptions = (cur: number): number[] => (FPS.includes(cur) ? FPS : [...FPS, cur].sort((a, b) => a - b));
export const bitrateOptions = (cur: number): number[] =>
  BITRATES.includes(cur) ? BITRATES : [...BITRATES, cur].sort((a, b) => a - b);

import { createDarkTheme, type BrandVariants, type Theme } from "@fluentui/react-components";
import { BRAND_TEXT, FILL, brandRamp } from "../brand";

export const FONT_BASE = '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';
export const FONT_DISPLAY = '"Segoe UI Variable Display", "Segoe UI", system-ui, sans-serif';

/** The app's own colours, the same on every page whatever the Windows accent: Fluent's dark theme
 *  on the ramp made from the brand's FILL (brand.ts), a deep blue. Fills are that blue with white on
 *  them; text and thin lines in the brand colour take lighter steps, which read on the
 *  dark window. What is on (the tiles, the switches) takes brand.css's --ex-on instead, and only the
 *  crossing edge is the gradient. */
export function makeTheme(color: string = FILL): Theme {
  const ramp = brandRamp(color);
  const text = {
    rest: ramp[BRAND_TEXT.rest],
    hover: ramp[BRAND_TEXT.hover],
    pressed: ramp[BRAND_TEXT.pressed],
    selected: ramp[BRAND_TEXT.selected],
  };
  const theme = createDarkTheme(ramp as unknown as BrandVariants);
  return {
    ...theme,
    // Hover goes darker and pressed darker still, as in Windows 11.
    colorBrandBackground: ramp[80],
    colorBrandBackgroundHover: ramp[70],
    colorBrandBackgroundPressed: ramp[60],
    colorBrandBackgroundSelected: ramp[70],
    colorCompoundBrandBackground: ramp[90],
    colorCompoundBrandBackgroundHover: ramp[80],
    colorCompoundBrandBackgroundPressed: ramp[70],
    colorNeutralForegroundOnBrand: "#ffffff",
    // Brand text (links, transparent buttons) in each state: steps that read on whatever surface
    // the state puts behind it (brand.ts BRAND_TEXT, tested).
    colorBrandForeground1: text.rest,
    colorBrandForeground2: text.hover,
    colorBrandForegroundLink: text.rest,
    colorBrandForegroundLinkHover: text.hover,
    colorBrandForegroundLinkPressed: text.pressed,
    colorBrandForegroundLinkSelected: text.selected,
    colorCompoundBrandForeground1: text.rest,
    colorCompoundBrandForeground1Hover: text.hover,
    colorCompoundBrandForeground1Pressed: text.pressed,
    colorNeutralForeground2BrandHover: text.hover,
    colorNeutralForeground2BrandPressed: text.pressed,
    colorNeutralForeground2BrandSelected: text.selected,
    colorNeutralForeground3BrandHover: text.hover,
    colorNeutralForeground3BrandPressed: text.pressed,
    colorNeutralForeground3BrandSelected: text.selected,
    colorCompoundBrandStroke: text.rest,
    colorCompoundBrandStrokeHover: text.hover,
    colorCompoundBrandStrokePressed: text.pressed,
    // The keyboard focus ring: a pale tint of the brand's blue rather than plain white.
    colorStrokeFocus2: ramp[140],
    fontFamilyBase: FONT_BASE,
  };
}

/** One theme for every page: it never changes while the app runs. */
export const THEME = makeTheme();

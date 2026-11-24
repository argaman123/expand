// Fluent gives every portal's mount node (a Tooltip, a Menu, a Popover, a Toaster) the classes of
// the FluentProvider above it, `className` included, plus `position: absolute; top: 0; left: 0;
// right: 0; z-index: 1000000` and `data-portal-node="true"` (@fluentui/react-portal
// usePortalMountNode; FluentProvider's `applyStylesToPortals` is on by default). A rule that gives
// the provider a box (`height: 100%`, a background, a layout) gives every portal that box too: an
// invisible layer over the whole page that takes the pointer from what is under it.
//
// This fails when a rule whose subject is a provider class could give a portal anything more than
// text styles, unless the rule leaves portals out with `:not([data-portal-node])`.
//
// Usage: node scripts/check-portal-css.mjs [dir]   (default: src)
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

/** The classes the pages give their FluentProvider, and Fluent's own. */
const PROVIDER_CLASSES = ["ss-provider", "ss-display", "ss-barpage", "fui-FluentProvider"];
/** Inherited text styles: harmless on a portal's mount node. */
const TEXT_ONLY = /^(color|font(-.*)?|line-height|letter-spacing|text-.*|user-select|-webkit-user-select|cursor|-webkit-font-smoothing|direction)$/;
const LEAVES_PORTALS_OUT = ":not([data-portal-node])";

function cssFiles(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return cssFiles(path);
    return name.endsWith(".css") ? [path] : [];
  });
}

/** Every style rule as [selector list, declarations], also inside @media and the like. */
function rules(css) {
  const out = [];
  const text = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const walk = (s) => {
    let i = 0;
    while (i < s.length) {
      const open = s.indexOf("{", i);
      if (open < 0) break;
      const head = s.slice(i, open).trim();
      let depth = 1;
      let j = open + 1;
      while (j < s.length && depth > 0) {
        if (s[j] === "{") depth++;
        else if (s[j] === "}") depth--;
        j++;
      }
      const body = s.slice(open + 1, j - 1);
      if (head.startsWith("@")) {
        if (!/^@(keyframes|font-face)/.test(head)) walk(body);
      } else {
        out.push([head, body]);
      }
      i = j;
    }
  };
  walk(text);
  return out;
}

/** Splits at top-level commas (not inside :not(), :has() and the like). */
function splitTop(s, sep) {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "(" || c === "[") depth++;
    else if (c === ")" || c === "]") depth--;
    else if (depth === 0 && sep(c)) {
      parts.push(s.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(s.slice(start));
  return parts.map((p) => p.trim()).filter(Boolean);
}

/** The compound selector a selector styles (its last one), with what is inside brackets cut out. */
function subject(selector) {
  const compounds = splitTop(selector, (c) => c === " " || c === ">" || c === "+" || c === "~");
  const last = compounds[compounds.length - 1] ?? "";
  let depth = 0;
  let own = "";
  for (const c of last) {
    if (c === "(") depth++;
    if (depth === 0) own += c;
    if (c === ")") depth--;
  }
  return { last, own };
}

function properties(body) {
  return body
    .split(";")
    .map((d) => d.split(":")[0].trim())
    .filter(Boolean);
}

const dir = process.argv[2] ?? "src";
const problems = [];
for (const file of cssFiles(dir)) {
  for (const [selectors, body] of rules(readFileSync(file, "utf8"))) {
    for (const selector of splitTop(selectors, (c) => c === ",")) {
      const { last, own } = subject(selector);
      const isProvider = PROVIDER_CLASSES.some((cls) => new RegExp(`\\.${cls}(?![\\w-])`).test(own));
      if (!isProvider || last.includes(LEAVES_PORTALS_OUT)) continue;
      const boxy = properties(body).filter((p) => !TEXT_ONLY.test(p));
      if (boxy.length > 0) {
        problems.push(`${relative(process.cwd(), file)}: "${selector}" gives Fluent's portals ${boxy.join(", ")}`);
      }
    }
  }
}

if (problems.length > 0) {
  console.error("Rules on a FluentProvider's class reach its portals too (a tooltip's would cover the page):");
  for (const p of problems) console.error(`  ${p}`);
  console.error(`Scope them with ${LEAVES_PORTALS_OUT}, or keep them to text styles.`);
  process.exit(1);
}
console.log("portal check: no provider rule gives Fluent's portals a box");

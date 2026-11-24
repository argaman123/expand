// Round 3 options for the owner to choose from. Dev only: the mock runtime (mockTauri.ts) imports
// this, so none of it is in a build.
// - `?wall=c`: the monitors' wallpaper as the still evening sky (look-options.css).
import "./look-options.css";

const wall = new URLSearchParams(location.search).get("wall");
if (wall === "c") document.documentElement.dataset.wall = wall;

export {};

// The top bar over the full-screen viewer: a slim handle that opens into a few controls while
// the pointer is over it. Where it sits and when it shows is decided by the app (bar.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const tab = document.querySelector<HTMLDivElement>("#tab")!;
const bar = document.querySelector<HTMLElement>("#bar")!;

let closing: number | undefined;

async function open() {
  window.clearTimeout(closing);
  if (!bar.hidden) return;
  await invoke("bar_expand", { expanded: true });
  tab.hidden = true;
  bar.hidden = false;
}

function closeSoon() {
  window.clearTimeout(closing);
  closing = window.setTimeout(async () => {
    bar.hidden = true;
    tab.hidden = false;
    await invoke("bar_expand", { expanded: false });
  }, 700);
}

// The app hid the bar (the viewer went away or lost focus): back to the handle.
void listen("bar-closed", () => {
  window.clearTimeout(closing);
  bar.hidden = true;
  tab.hidden = false;
});

document.body.addEventListener("mouseenter", open);
document.body.addEventListener("mouseleave", closeSoon);

function on(id: string, command: string) {
  document.querySelector<HTMLButtonElement>(`#${id}`)!.addEventListener("click", async () => {
    try {
      await invoke(command);
    } finally {
      closeSoon();
    }
  });
}

on("settings", "bar_settings");
on("windowed", "bar_windowed");
on("move", "bar_move");
on("disconnect", "disconnect");

// What of the bar shows, and when it shows by itself. Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { PEEK_MS, barTarget, type BarState } from "../src/bar/peek.ts";

const idle: BarState = { popup: false, popOpen: false, open: false, menu: false, near: false, peeking: false };

test("with nothing going on, nothing shows", () => {
  assert.equal(barTarget(idle), "hidden");
});

test("the pointer near the top centre brings the whole bar, with no tab step (round 3)", () => {
  assert.equal(barTarget({ ...idle, near: true }), "bar");
});

test("while it peeks, the whole bar shows by itself, the pointer far away", () => {
  assert.equal(barTarget({ ...idle, peeking: true }), "bar");
});

test("open, the bar shows; with a menu or popover, all of the window", () => {
  assert.equal(barTarget({ ...idle, open: true, peeking: true }), "bar");
  assert.equal(barTarget({ ...idle, open: true, menu: true }), "menu");
  assert.equal(barTarget({ ...idle, near: true, menu: true, open: true }), "menu");
});

test("holding the title bar's popover, only the popover shows, never the bar", () => {
  assert.equal(barTarget({ ...idle, popup: true, near: true, peeking: true }), "hidden");
  assert.equal(barTarget({ ...idle, popup: true, popOpen: true }), "menu");
});

test("a peek lasts about three seconds", () => {
  assert.ok(PEEK_MS >= 2500 && PEEK_MS <= 3500);
});

test("after a peek the bar goes, unless the pointer is near", () => {
  assert.equal(barTarget({ ...idle, peeking: false }), "hidden");
  assert.equal(barTarget({ ...idle, peeking: false, near: true }), "bar");
});

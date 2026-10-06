// Gestures on a phone, played with made-up touch events: what reaches the host.
// Run with `npm test` (Node's own test runner, no browser needed).

import { test, beforeEach, afterEach, mock } from "node:test";
import assert from "node:assert/strict";
import { TouchControl } from "./touch.ts";

type Finger = { clientX: number; clientY: number };

let sent: unknown[];
let views: [number, number, number][];
let touch: TouchControl;

/** A touch event with these fingers still on the screen. */
function ev(...fingers: Finger[]) {
  return { touches: fingers, preventDefault() {} } as unknown as TouchEvent;
}
const at = (x: number, y: number): Finger => ({ clientX: x, clientY: y });

const down = (button: string) => ({ MouseButton: { button, down: true } });
const up = (button: string) => ({ MouseButton: { button, down: false } });
const move = (x: number, y: number) => ({ MouseMove: { x, y } });

beforeEach(() => {
  mock.timers.enable({ apis: ["setTimeout"] });
  sent = [];
  views = [];
  touch = new TouchControl({
    // Remote pixels are screen pixels here.
    toRemote: (x, y) => ({ x, y }),
    send: (e) => sent.push(e),
    view: (zoom, x, y) => views.push([zoom, x, y]),
  });
});

afterEach(() => mock.timers.reset());

test("tap clicks where the finger was", () => {
  touch.onStart(ev(at(100, 200)));
  touch.onEnd(ev());
  assert.deepEqual(sent, [move(100, 200), down("Left"), up("Left")]);
});

test("a little wobble is still a tap", () => {
  touch.onStart(ev(at(100, 200)));
  touch.onMove(ev(at(104, 203)));
  touch.onEnd(ev());
  assert.deepEqual(sent, [move(100, 200), down("Left"), up("Left")]);
});

test("long press right-clicks, and lifting adds nothing", () => {
  touch.onStart(ev(at(50, 60)));
  mock.timers.tick(549);
  assert.deepEqual(sent, []);
  mock.timers.tick(1);
  assert.deepEqual(sent, [move(50, 60), down("Right"), up("Right")]);
  touch.onEnd(ev());
  assert.equal(sent.length, 3);
});

test("drag holds the left button from the starting point", () => {
  touch.onStart(ev(at(10, 10)));
  touch.onMove(ev(at(40, 10)));
  touch.onMove(ev(at(80, 30)));
  touch.onEnd(ev());
  assert.deepEqual(sent, [move(10, 10), down("Left"), move(40, 10), move(80, 30), up("Left")]);
  // No right click once the finger moved.
  mock.timers.tick(1000);
  assert.equal(sent.length, 5);
});

test("two fingers moving down scroll up the page, without clicks", () => {
  touch.onStart(ev(at(100, 100), at(200, 100)));
  touch.onMove(ev(at(100, 115), at(200, 115)));
  touch.onMove(ev(at(100, 125), at(200, 125)));
  touch.onEnd(ev(at(200, 125)));
  touch.onEnd(ev());
  assert.ok(sent.length > 0);
  for (const e of sent) assert.ok("Wheel" in (e as object), `only wheel events, got ${JSON.stringify(e)}`);
  const dy = sent.reduce((sum: number, e) => sum + (e as { Wheel: { dy: number } }).Wheel.dy, 0);
  assert.ok(dy > 0, "positive wheel is up, as fingers pulling the page down");
  assert.deepEqual(views, []);
});

test("spreading two fingers zooms the view, not the host", () => {
  touch.onStart(ev(at(100, 100), at(200, 100)));
  touch.onMove(ev(at(50, 100), at(250, 100)));
  touch.onEnd(ev(at(250, 100)));
  touch.onEnd(ev());
  assert.deepEqual(sent, []);
  const [zoom] = views.at(-1)!;
  assert.equal(zoom, 2);
});

test("zoom stays between whole screen and 5×, and resets", () => {
  touch.onStart(ev(at(100, 100), at(110, 100)));
  touch.onMove(ev(at(0, 100), at(1000, 100)));
  assert.equal(views.at(-1)![0], 5);
  touch.onMove(ev(at(104, 100), at(106, 100)));
  assert.deepEqual(views.at(-1), [1, 0, 0]);
  touch.onEnd(ev());
  touch.resetView();
  assert.deepEqual(views.at(-1), [1, 0, 0]);
});

test("the point under the fingers stays there while zooming", () => {
  touch.onStart(ev(at(100, 100), at(200, 100)));
  touch.onMove(ev(at(50, 100), at(250, 100)));
  const [zoom, x, y] = views.at(-1)!;
  // Midpoint (150, 100) of the unzoomed view maps to itself.
  assert.equal(150 * zoom + x, 150);
  assert.equal(100 * zoom + y, 100);
});

test("a second finger during a drag lets go of the button", () => {
  touch.onStart(ev(at(10, 10)));
  touch.onMove(ev(at(60, 10)));
  touch.onStart(ev(at(60, 10), at(120, 10)));
  assert.deepEqual(sent.at(-1), up("Left"));
  touch.onEnd(ev(at(120, 10)));
  touch.onEnd(ev());
  assert.equal(sent.filter((e) => JSON.stringify(e) === JSON.stringify(up("Left"))).length, 1);
});

test("lifting the fingers of a two-finger gesture one by one does not click", () => {
  touch.onStart(ev(at(100, 100), at(200, 100)));
  touch.onEnd(ev(at(200, 100)));
  touch.onEnd(ev());
  mock.timers.tick(1000);
  assert.deepEqual(sent, []);
});

test("three fingers do nothing", () => {
  touch.onStart(ev(at(1, 1), at(2, 2), at(3, 3)));
  touch.onMove(ev(at(50, 50), at(60, 60), at(70, 70)));
  touch.onEnd(ev());
  mock.timers.tick(1000);
  assert.deepEqual(sent, []);
});

test("after a gesture, the next tap works again", () => {
  touch.onStart(ev(at(100, 100), at(200, 100)));
  touch.onEnd(ev());
  touch.onStart(ev(at(30, 40)));
  touch.onEnd(ev());
  assert.deepEqual(sent, [move(30, 40), down("Left"), up("Left")]);
});

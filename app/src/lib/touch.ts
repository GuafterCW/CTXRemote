/**
 * Touch control of the remote screen on a phone or tablet:
 * - tap: left click where the finger was
 * - long press: right click
 * - drag with one finger: move with the left button held (select, move windows)
 * - two fingers moving together: scroll
 * - two fingers spreading or pinching: zoom the local view; while zoomed,
 *   moving both fingers pans it
 */

import type { InputEvent } from "./api";

type Point = { x: number; y: number };

export interface TouchTarget {
  /** Remote pixel position of a point on the screen. */
  toRemote(clientX: number, clientY: number): Point;
  send(event: InputEvent): void;
  /** The local view's zoom (1 = fit) and offset in CSS pixels. */
  view(zoom: number, panX: number, panY: number): void;
}

const LONG_PRESS_MS = 550;
/** Finger movement in CSS pixels before a touch counts as a drag. */
const SLOP = 10;
/** Relative change of the finger distance that makes two fingers a zoom. */
const ZOOM_START = 0.12;
const MAX_ZOOM = 5;
/** Windows wheel units per CSS pixel of two-finger movement. */
const SCROLL_FACTOR = 4;

type Mode = "idle" | "pending" | "drag" | "two" | "ignore";

export class TouchControl {
  private mode: Mode = "idle";
  private start: Point & { clientX: number; clientY: number } = { x: 0, y: 0, clientX: 0, clientY: 0 };
  private longTimer: ReturnType<typeof setTimeout> | undefined;
  private zoom = 1;
  private pan = { x: 0, y: 0 };
  private gesture = { dist: 1, mid: { x: 0, y: 0 }, lastMid: { x: 0, y: 0 }, zoom: 1, pan: { x: 0, y: 0 }, kind: null as null | "zoom" | "scroll" };
  private wheel = { dx: 0, dy: 0 };

  private target: TouchTarget;

  constructor(target: TouchTarget) {
    this.target = target;
  }

  /** Back to the whole screen. */
  resetView() {
    this.zoom = 1;
    this.pan = { x: 0, y: 0 };
    this.target.view(1, 0, 0);
  }

  onStart(e: TouchEvent) {
    // No emulated mouse events afterwards: everything is handled here.
    e.preventDefault();
    if (e.touches.length === 1 && this.mode === "idle") {
      const t = e.touches[0];
      const p = this.target.toRemote(t.clientX, t.clientY);
      this.start = { ...p, clientX: t.clientX, clientY: t.clientY };
      this.mode = "pending";
      clearTimeout(this.longTimer);
      this.longTimer = setTimeout(() => {
        if (this.mode !== "pending") return;
        this.click("Right");
        this.mode = "ignore";
        navigator.vibrate?.(15);
      }, LONG_PRESS_MS);
    } else if (e.touches.length === 2) {
      clearTimeout(this.longTimer);
      if (this.mode === "drag") this.target.send({ MouseButton: { button: "Left", down: false } });
      const [a, b] = [e.touches[0], e.touches[1]];
      const mid = { x: (a.clientX + b.clientX) / 2, y: (a.clientY + b.clientY) / 2 };
      this.gesture = { dist: distance(a, b), mid, lastMid: mid, zoom: this.zoom, pan: { ...this.pan }, kind: null };
      this.mode = "two";
    } else {
      this.mode = "ignore";
    }
  }

  onMove(e: TouchEvent) {
    e.preventDefault();
    if (this.mode === "pending" || this.mode === "drag") {
      const t = e.touches[0];
      if (!t) return;
      if (this.mode === "pending") {
        if (Math.hypot(t.clientX - this.start.clientX, t.clientY - this.start.clientY) < SLOP) return;
        clearTimeout(this.longTimer);
        this.mode = "drag";
        this.target.send({ MouseMove: { x: this.start.x, y: this.start.y } });
        this.target.send({ MouseButton: { button: "Left", down: true } });
      }
      this.target.send({ MouseMove: this.target.toRemote(t.clientX, t.clientY) });
    } else if (this.mode === "two" && e.touches.length >= 2) {
      const [a, b] = [e.touches[0], e.touches[1]];
      const mid = { x: (a.clientX + b.clientX) / 2, y: (a.clientY + b.clientY) / 2 };
      const ratio = distance(a, b) / this.gesture.dist;
      const g = this.gesture;
      if (g.kind === null) {
        if (Math.abs(ratio - 1) > ZOOM_START) g.kind = "zoom";
        else if (Math.hypot(mid.x - g.mid.x, mid.y - g.mid.y) > SLOP) g.kind = this.zoom > 1 ? "zoom" : "scroll";
      }
      if (g.kind === "zoom") {
        const zoom = Math.min(MAX_ZOOM, Math.max(1, g.zoom * ratio));
        // The point under the fingers stays under them.
        const scale = zoom / g.zoom;
        this.zoom = zoom;
        this.pan = zoom === 1 ? { x: 0, y: 0 } : { x: mid.x - (g.mid.x - g.pan.x) * scale, y: mid.y - (g.mid.y - g.pan.y) * scale };
        this.target.view(this.zoom, this.pan.x, this.pan.y);
      } else if (g.kind === "scroll") {
        // Fingers moving down scroll up, as on a touch screen.
        this.wheel.dy += (mid.y - g.lastMid.y) * SCROLL_FACTOR;
        this.wheel.dx -= (mid.x - g.lastMid.x) * SCROLL_FACTOR;
        const dx = Math.trunc(this.wheel.dx);
        const dy = Math.trunc(this.wheel.dy);
        if (dx || dy) {
          this.wheel = { dx: this.wheel.dx - dx, dy: this.wheel.dy - dy };
          this.target.send({ Wheel: { dx, dy } });
        }
      }
      g.lastMid = mid;
    }
  }

  onEnd(e: TouchEvent) {
    e.preventDefault();
    if (e.touches.length > 0) {
      // One of two fingers lifted: wait until all are gone.
      if (this.mode === "two") this.mode = "ignore";
      return;
    }
    clearTimeout(this.longTimer);
    if (this.mode === "pending") this.click("Left");
    if (this.mode === "drag") this.target.send({ MouseButton: { button: "Left", down: false } });
    this.mode = "idle";
  }

  private click(button: "Left" | "Right") {
    this.target.send({ MouseMove: { x: this.start.x, y: this.start.y } });
    this.target.send({ MouseButton: { button, down: true } });
    this.target.send({ MouseButton: { button, down: false } });
  }
}

function distance(a: Touch, b: Touch) {
  return Math.max(1, Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY));
}

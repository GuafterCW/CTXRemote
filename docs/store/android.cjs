const { chromium } = require('/opt/node-tools/node_modules/playwright');
const fs = require('fs');
// Usage: node android.cjs <folder with desktop.png> [port of a server for app/dist]
// Renders the phone screenshots for Google Play into that folder and checks
// the touch control of a session on the real page: Chromium with touch input
// and an Android user agent, so the app takes its phone layout.
const dir = process.argv[2];
const port = process.argv[3] || '8765';
const desktop = 'data:image/png;base64,' + fs.readFileSync(dir + '/desktop.png').toString('base64');
const UA = 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36';
const mock = ({ label, desktop }) => {
  const handlers = {}; let next = 1;
  const now = Math.floor(Date.now() / 1000);
  window.__fire = (event, payload) => Object.values(handlers).forEach(h => h.event === event && h.fn({ event, id: 0, payload }));
  // Stands in for the H.264 decoder: every chunk shows the prepared desktop picture.
  const img = new Image(); img.src = desktop;
  window.VideoDecoder = class { constructor(o) { this.o = o; this.state = 'configured'; this.decodeQueueSize = 0; }
    configure() {} close() { this.state = 'closed'; }
    decode() { const go = () => this.o.output(new VideoFrame(img, { timestamp: 0 })); img.complete ? go() : img.onload = go; } };
  const peer = (id, alias, name, online, tags = []) => ({ id, alias, name, lastSeen: now - 3600, access: true, wake: true, tags, online });
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label }, currentWebview: { windowLabel: label, label } },
    transformCallback: (fn) => { const id = next++; window["_" + id] = fn; return id; },
    unregisterCallback: () => {},
    invoke: async (cmd, args) => {
      if (cmd === "plugin:event|listen") { handlers[args.handler] = { event: args.event, fn: window["_" + args.handler] }; return args.handler; }
      (window.__calls ||= []).push([cmd, args]);
      if (cmd.startsWith("plugin:")) return null;
      switch (cmd) {
        case "session_title": return args.session === 1 ? "Büro-PC · 482 913 077" : "Laptop Anna · 615 220 948";
        case "attach": {
          const ch = Object.values(args).find(v => v && typeof v === 'object' && 'id' in v);
          setTimeout(() => { const b = new ArrayBuffer(16); const v = new DataView(b); v.setUint8(0, 1); v.setUint8(1, 1); v.setUint32(4, 1920, true); v.setUint32(8, 1080, true); window["_" + ch.id]({ index: 0, message: b }); }, 100);
          return { host: { hostname: "buero-pc", username: "anna", os: "Windows 11", displays: [{ index: 0, name: "D1", width: 1920, height: 1080, primary: true }], active_display: 0 }, id: "482 913 077", label: "Büro-PC", features: { files: true, restart: true, quality: true, chat: true, draw: true, recording: true, audio: false, typeText: true }, direct: "192.168.1.20:21301", rights: 127, privacy: false, lockOnEnd: false };
        }
        case "overview": return { presence: { state: "online", id: "731 004 552" }, password: "k3x9pq7m", server: "remote.remocto.com", unattended: true, service: true, hostSupported: true, hostLimitation: null,
          peers: [peer("482913077", "Büro-PC", "BUERO-PC", true, ["Firma"]), peer("615220948", "Laptop Anna", "ANNA-LAPTOP", true, ["Familie"]), peer("903117254", "Server Keller", "NAS01", false, ["Firma"]), peer("277450193", "Praxis Empfang", "EMPFANG-01", true, ["Kunden"]), peer("118604377", "Opa Heinz", "HEINZ-PC", false, ["Familie"])],
          hosted: [], rightsAttended: 31, rightsUnattended: 127, version: "0.1.48", update: null, publicAlias: "philipp", aliasSupported: true, profile: null, account: null, direct: { enabled: true, port: 21301, addresses: [] }, directActive: true };
        case "local_home": return "C:\\Users\\philipp\\Downloads";
        case "local_list": return { path: "C:\\Users\\philipp\\Downloads", parent: "C:\\Users\\philipp", entries: [
          { name: "Projekte", path: "C:\\p", kind: "Dir", size: 0, modified: now - 3600 },
          { name: "Angebot-Hofmann.pdf", path: "C:\\Users\\philipp\\Downloads\\Angebot-Hofmann.pdf", kind: "File", size: 482113, modified: now - 86400 },
          { name: "Treiber-Drucker.zip", path: "C:\\t.zip", kind: "File", size: 73400320, modified: now - 900000 } ] };
        case "remote_list": return { path: "C:\\Users\\anna\\Documents", parent: "C:\\Users\\anna", entries: [
          { name: "Buchhaltung", path: "C:\\b", kind: "Dir", size: 0, modified: now - 7200 },
          { name: "Auftrag-A-1042.docx", path: "C:\\x.docx", kind: "File", size: 23111, modified: now - 7200 },
          { name: "Kundenliste.xlsx", path: "C:\\k.xlsx", kind: "File", size: 96400, modified: now - 172800 },
          { name: "Datensicherung-Oktober.zip", path: "C:\\d.zip", kind: "File", size: 734003200, modified: now - 50000 } ] };
        case "take_drops": return [];
        case "upload": return 7;
        case "download": return 8;
        case "history": return [];
        case "separate_windows": return false;
        case "send_input": (window.__inputs ||= []).push(args.event); return null;
      }
      return null;
    },
  };
};

let failed = 0;
const check = (ok, what, detail) => { console.log((ok ? 'ok   ' : 'FAIL ') + what + (ok || detail === undefined ? '' : ' ' + JSON.stringify(detail))); if (!ok) failed++; };

(async () => {
  const b = await chromium.launch();
  const out = dir;
  fs.mkdirSync(out, { recursive: true });
  const page = async (hash, width, height) => {
    const ctx = await b.newContext({ viewport: { width, height }, deviceScaleFactor: 2.625, isMobile: true, hasTouch: true, userAgent: UA, colorScheme: 'dark' });
    const p = await ctx.newPage();
    p.on('pageerror', e => { console.log(hash, 'pageerror', e.message); failed++; });
    await p.addInitScript(mock, { label: 'main', desktop });
    await p.goto(`http://localhost:${port}/index.html` + hash);
    await p.waitForTimeout(900);
    return p;
  };

  // Start page, portrait (9:16, 1081×1922).
  let p = await page('#/', 412, 732);
  check(await p.evaluate(() => document.body.classList.contains('mobile')), 'phone layout active');
  await p.screenshot({ path: out + '/1-start.png' });
  await p.close();

  // Session, landscape like the remote screen.
  p = await page('#/session/1', 732, 412);
  await p.waitForTimeout(600);
  await p.screenshot({ path: out + '/2-sitzung.png' });
  const box = await p.locator('canvas').first().boundingBox();
  check(!!box && box.width > 300, 'remote picture shown', box);
  const cdp = await p.context().newCDPSession(p);
  const touch = (type, pts) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: pts.map(([x, y], id) => ({ x, y, id })) });
  const take = () => p.evaluate(() => { const i = window.__inputs || []; window.__inputs = []; return i; });
  const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
  await take();

  // Tap in the middle: left click near the middle of the 1920×1080 screen.
  await touch('touchStart', [[cx, cy]]); await p.waitForTimeout(60); await touch('touchEnd', []);
  await p.waitForTimeout(100);
  let ev = await take();
  const mv = ev.find(e => e.MouseMove);
  check(mv && Math.abs(mv.MouseMove.x - 960) < 20 && Math.abs(mv.MouseMove.y - 540) < 20, 'tap moves to the middle', mv);
  check(JSON.stringify(ev.filter(e => e.MouseButton)) === JSON.stringify([{ MouseButton: { button: 'Left', down: true } }, { MouseButton: { button: 'Left', down: false } }]), 'tap is a left click', ev);

  // Tap top-left corner of the picture.
  await touch('touchStart', [[box.x + 2, box.y + 2]]); await touch('touchEnd', []);
  await p.waitForTimeout(100);
  ev = await take();
  const c = ev.find(e => e.MouseMove);
  check(c && c.MouseMove.x < 20 && c.MouseMove.y < 20, 'tap in the corner hits the corner', c);

  // Long press: right click.
  await touch('touchStart', [[cx, cy]]); await p.waitForTimeout(700); await touch('touchEnd', []);
  await p.waitForTimeout(100);
  ev = await take();
  check(ev.some(e => e.MouseButton?.button === 'Right' && e.MouseButton.down) && !ev.some(e => e.MouseButton?.button === 'Left'), 'long press is a right click', ev);

  // Drag: left button held while moving, released at the end.
  await touch('touchStart', [[cx, cy]]);
  for (let i = 1; i <= 5; i++) { await touch('touchMove', [[cx + i * 20, cy]]); await p.waitForTimeout(20); }
  await touch('touchEnd', []);
  await p.waitForTimeout(100);
  ev = await take();
  const btn = ev.filter(e => e.MouseButton).map(e => e.MouseButton.down);
  check(JSON.stringify(btn) === '[true,false]' && ev.filter(e => e.MouseMove).length >= 5, 'drag holds the left button', ev);

  // Two fingers moving down together: scroll, no clicks.
  await touch('touchStart', [[cx - 40, cy - 60], [cx + 40, cy - 60]]);
  for (let i = 1; i <= 6; i++) { await touch('touchMove', [[cx - 40, cy - 60 + i * 15], [cx + 40, cy - 60 + i * 15]]); await p.waitForTimeout(20); }
  await touch('touchEnd', []);
  await p.waitForTimeout(100);
  ev = await take();
  const dy = ev.filter(e => e.Wheel).reduce((s, e) => s + e.Wheel.dy, 0);
  check(dy > 0 && !ev.some(e => e.MouseButton), 'two fingers down scroll up', ev);

  // Spread: the picture zooms locally, nothing goes to the host.
  await touch('touchStart', [[cx - 30, cy], [cx + 30, cy]]);
  for (let i = 1; i <= 6; i++) { await touch('touchMove', [[cx - 30 - i * 15, cy], [cx + 30 + i * 15, cy]]); await p.waitForTimeout(20); }
  await touch('touchEnd', []);
  await p.waitForTimeout(150);
  ev = await take();
  const tf = await p.evaluate(() => [...document.querySelectorAll('[style*="scale("]')].map(e => e.style.transform)[0] || '');
  check(/scale\(([2-5](\.\d+)?)\)/.test(tf) && ev.length === 0, 'spreading zooms the picture', { tf, ev });
  check(await p.locator('[title="Ganzes Bild"]').count() === 1, '"Ganzes Bild" button appears when zoomed');

  // While zoomed, a tap in the middle still hits the same remote point.
  await touch('touchStart', [[cx, cy]]); await touch('touchEnd', []);
  await p.waitForTimeout(100);
  ev = await take();
  const z = ev.find(e => e.MouseMove);
  check(z && Math.abs(z.MouseMove.x - 960) < 20 && Math.abs(z.MouseMove.y - 540) < 20, 'tap when zoomed hits the point under the finger', z);
  await p.screenshot({ path: out + '/3-zoom.png' });
  await p.locator('[title="Ganzes Bild"]').tap();
  await p.waitForTimeout(150);
  check(await p.locator('[title="Ganzes Bild"]').count() === 0, '"Ganzes Bild" resets the zoom');

  // Keyboard: typed text goes as text, the extra keys as keys.
  await p.locator('[title="Tastatur"]').tap();
  await p.waitForTimeout(200);
  check(await p.locator('textarea.typing').count() === 1, 'keyboard field opens');
  await p.screenshot({ path: out + '/4-tastatur.png' });
  await take();
  await p.locator('textarea.typing').pressSequentially('Hallo ä');
  await p.waitForTimeout(100);
  ev = await take();
  const text = ev.filter(e => e.Text).map(e => e.Text).join('');
  check(text === 'Hallo ä', 'typing sends the text', ev);
  await p.locator('.extra-keys button', { hasText: 'Strg' }).tap();
  await p.locator('textarea.typing').pressSequentially('c');
  await p.waitForTimeout(100);
  ev = await take();
  const keys = ev.filter(e => e.Key).map(e => `${e.Key.code}${e.Key.down ? '↓' : '↑'}`).join(' ');
  check(keys.includes('ControlLeft↓') && keys.includes('KeyC↓') && keys.includes('ControlLeft↑'), 'Strg then c is Ctrl+C', ev);
  await p.locator('.extra-keys button', { hasText: 'Esc' }).tap();
  await p.waitForTimeout(100);
  ev = await take();
  check(ev.some(e => e.Key?.code === 'Escape' && e.Key.down), 'Esc key', ev);
  await p.close();

  // Files on the phone, portrait.
  p = await page('#/files/1', 412, 732);
  await p.screenshot({ path: out + '/5-dateien.png' });
  await p.close();

  await b.close();
  console.log(failed ? `${failed} failed` : 'all passed');
  process.exit(failed ? 1 : 0);
})();

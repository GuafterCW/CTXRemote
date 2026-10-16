const { chromium } = require('/opt/node-tools/node_modules/playwright');
const fs = require('fs');
const dir = process.argv[2];
const desktop = 'data:image/png;base64,' + fs.readFileSync(dir + '/desktop.png').toString('base64');
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
      if (cmd.startsWith("plugin:")) return null;
      switch (cmd) {
        case "session_title": return args.session === 1 ? "Büro-PC · 482 913 077" : "Laptop Anna · 615 220 948";
        case "attach": {
          const ch = Object.values(args).find(v => v && typeof v === 'object' && 'id' in v);
          setTimeout(() => { const b = new ArrayBuffer(16); const v = new DataView(b); v.setUint8(0, 1); v.setUint8(1, 1); v.setUint32(4, 1920, true); v.setUint32(8, 1080, true); window["_" + ch.id]({ index: 0, message: b }); }, 100);
          return { host: { hostname: "buero-pc", username: "anna", os: "Windows 11", displays: [{ index: 0, name: "D1", width: 1920, height: 1080, primary: true }], active_display: 0 }, id: "482 913 077", label: "Büro-PC", features: { files: true, restart: true, quality: true, chat: true, draw: true, recording: true, audio: false }, direct: "192.168.1.20:21301", rights: 127, privacy: false, lockOnEnd: false };
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
      }
      return null;
    },
  };
};
(async () => {
  const b = await chromium.launch();
  const page = async (label, hash) => {
    const p = await b.newPage({ viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1.5, colorScheme: 'dark' });
    p.on('pageerror', e => console.log(hash, 'pageerror', e.message));
    await p.addInitScript(mock, { label, desktop });
    await p.goto('http://localhost:8765/index.html' + hash);
    await p.waitForTimeout(900);
    return p;
  };
  let p = await page('main', '#/');
  await p.screenshot({ path: dir + '/1-start.png' });
  await p.close();

  p = await page('sessions', '#/tabs/1');
  await p.evaluate(() => window.__fire("tab-open", { session: 2, title: "Laptop Anna · 615 220 948" }));
  await p.waitForTimeout(400);
  await p.locator('.tab .pick').first().click();
  await p.waitForTimeout(400);
  await p.mouse.move(640, 70); await p.waitForTimeout(400);
  await p.screenshot({ path: dir + '/2-sitzung.png' });
  await p.locator('button:has-text("Chat"), [aria-label="Chat"], [title="Chat"]').first().click().catch(e => console.log('chat', e.message));
  await p.waitForTimeout(200);
  await p.evaluate(() => window.__fire("chat-1", "Hallo! Der Drucker im Büro druckt nichts mehr."));
  await p.waitForTimeout(200);
  await p.keyboard.type("Ich schaue es mir gleich an.").catch(() => {});
  await p.screenshot({ path: dir + '/3-chat.png' });
  await p.close();

  p = await page('files-1', '#/files/1');
  await p.click('text=Angebot-Hofmann.pdf').catch(e => console.log(e.message));
  await p.click('button:has-text("Hochladen")').catch(() => {});
  await p.evaluate(() => window.__fire("transfer", { session: 1, kind: "progress", id: 7, done: 310000, total: 482113 }));
  await p.waitForTimeout(300);
  await p.screenshot({ path: dir + '/4-dateien.png' });
  await p.close();

  p = await page('main', '#/');
  await p.locator('[title="Einstellungen"], [aria-label="Einstellungen"]').first().click().catch(e => console.log('settings', e.message));
  await p.waitForTimeout(500);
  await p.locator('text=Rechte der Gegenseite').first().scrollIntoViewIfNeeded().catch(e => console.log('scroll', e.message));
  await p.evaluate(() => { const h = [...document.querySelectorAll('*')].find(e => e.textContent.trim() === 'Rechte der Gegenseite'); let el = h; while (el && el.scrollHeight <= el.clientHeight) el = el.parentElement; if (el) el.scrollTop = h.offsetTop - 120; });
  await p.waitForTimeout(300);
  await p.screenshot({ path: dir + '/5-einstellungen.png' });
  await p.close();
  await b.close();
})();

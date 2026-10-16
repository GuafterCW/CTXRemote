// Google Play graphics from the app icon: icon 512×512 and feature graphic 1024×500.
// node docs/store/play.cjs app/app-icon.svg docs/store/android
const { chromium } = require('/opt/node-tools/node_modules/playwright');
const fs = require('fs');
const [svgPath, out] = process.argv.slice(2);
const svg = fs.readFileSync(svgPath, 'utf8');
const sized = (size) => svg.replace('<svg', `<svg width="${size}" height="${size}" style="display:block"`);
const feature = `<html><body style="margin:0">
<div style="width:1024px;height:500px;box-sizing:border-box;padding:0 84px;display:flex;align-items:center;gap:56px;
  font-family:'Segoe UI',system-ui,sans-serif;color:#fff;
  background:radial-gradient(circle at 18% 30%,#1f7a63 0,transparent 55%),radial-gradient(circle at 90% 90%,#0f4a3d 0,transparent 60%),#0b2e26">
  <div style="flex:none;filter:drop-shadow(0 18px 40px rgba(0,0,0,.35))">${sized(240)}</div>
  <div>
    <div style="font-size:84px;font-weight:700;letter-spacing:-2px;line-height:1">Remocto</div>
    <div style="font-size:30px;margin-top:22px;line-height:1.3;color:#cfe9df">Fernwartung, Ende-zu-Ende verschlüsselt.<br>Den PC vom Handy aus steuern.</div>
  </div>
</div></body></html>`;
(async () => {
  const b = await chromium.launch();
  let p = await b.newPage({ viewport: { width: 512, height: 512 } });
  // Play wants a full square (it rounds the corners itself): fill them with the icon's colour.
  await p.setContent(`<html><body style="margin:0;background:#16211e">${sized(512)}</body></html>`);
  await p.screenshot({ path: `${out}/icon-512.png` });
  p = await b.newPage({ viewport: { width: 1024, height: 500 } });
  await p.setContent(feature);
  await p.screenshot({ path: `${out}/feature-1024x500.png` });
  await b.close();
})();

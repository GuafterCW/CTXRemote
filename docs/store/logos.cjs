const { chromium } = require('/opt/node-tools/node_modules/playwright');
const fs = require('fs');
const [svgPath, out] = process.argv.slice(2);
const svg = fs.readFileSync(svgPath, 'utf8');
(async () => {
  const b = await chromium.launch();
  for (const size of [2160, 1080, 300, 150, 71]) {
    const p = await b.newPage({ viewport: { width: size, height: size } });
    await p.setContent(`<html><body style="margin:0;background:transparent">${svg.replace('<svg', `<svg width="${size}" height="${size}" style="display:block"`)}</body></html>`);
    await p.screenshot({ path: `${out}/logo-${size}.png`, omitBackground: true });
    await p.close();
  }
  await b.close();
})();

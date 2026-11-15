// Renders the app icon (docs/art/icon/*.svg) into the files the app and its packages carry,
// in crates/dstudio-app/icons/:
//
//   device.icns      macOS bundle icon (16 to 1024 px, from app-icon-macos.svg)
//   device.ico       Windows executable and installer icon (16 to 256 px, from app-icon.svg)
//   device-256.png   window icon (Windows, Linux) and the Linux desktop entry's icon
//   device-macos-512.png  Dock icon when the app runs outside its bundle (cargo run)
//
//   npm install --no-save playwright && npx playwright install chromium   (once)
//   node tools/art/make-icons.mjs
//
// The .icns and .ico containers hold PNG images, which macOS 10.7+ and Windows Vista+ read.
import { chromium } from "playwright";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const src = join(root, "docs", "art", "icon");
const out = join(root, "crates", "dstudio-app", "icons");
mkdirSync(out, { recursive: true });

const browser = await chromium.launch();
async function render(svgFile, size) {
  const svg = readFileSync(join(src, svgFile), "utf8");
  const page = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
  await page.setContent(
    `<html><body style="margin:0;background:transparent">` +
      `<div id="i" style="width:${size}px;height:${size}px">${svg.replace("<svg ", `<svg width="${size}" height="${size}" `)}</div>` +
      `</body></html>`,
  );
  const png = await page.locator("#i").screenshot({ omitBackground: true });
  await page.close();
  return png;
}

// ICNS: "icns" + total length, then (type, length, PNG) entries.
const icnsTypes = [
  ["icp4", 16], ["icp5", 32], ["icp6", 64], ["ic07", 128], ["ic08", 256], ["ic09", 512], ["ic10", 1024],
  ["ic11", 32], ["ic12", 64], ["ic13", 256], ["ic14", 512],
];
const mac = new Map();
for (const [, size] of icnsTypes) if (!mac.has(size)) mac.set(size, await render("app-icon-macos.svg", size));
const entries = icnsTypes.map(([type, size]) => {
  const png = mac.get(size);
  const head = Buffer.alloc(8);
  head.write(type, 0, "ascii");
  head.writeUInt32BE(png.length + 8, 4);
  return Buffer.concat([head, png]);
});
const body = Buffer.concat(entries);
const icnsHead = Buffer.alloc(8);
icnsHead.write("icns", 0, "ascii");
icnsHead.writeUInt32BE(body.length + 8, 4);
writeFileSync(join(out, "device.icns"), Buffer.concat([icnsHead, body]));
writeFileSync(join(out, "device-macos-512.png"), mac.get(512));

// ICO: ICONDIR, one ICONDIRENTRY per size, then the PNGs.
const icoSizes = [16, 24, 32, 48, 64, 128, 256];
const pngs = [];
for (const s of icoSizes) pngs.push(await render("app-icon.svg", s));
const dir = Buffer.alloc(6 + 16 * icoSizes.length);
dir.writeUInt16LE(0, 0);
dir.writeUInt16LE(1, 2);
dir.writeUInt16LE(icoSizes.length, 4);
let offset = dir.length;
icoSizes.forEach((s, i) => {
  const e = 6 + 16 * i;
  dir.writeUInt8(s >= 256 ? 0 : s, e);
  dir.writeUInt8(s >= 256 ? 0 : s, e + 1);
  dir.writeUInt16LE(1, e + 4); // planes
  dir.writeUInt16LE(32, e + 6); // bits per pixel
  dir.writeUInt32LE(pngs[i].length, e + 8);
  dir.writeUInt32LE(offset, e + 12);
  offset += pngs[i].length;
});
writeFileSync(join(out, "device.ico"), Buffer.concat([dir, ...pngs]));
writeFileSync(join(out, "device-256.png"), pngs[icoSizes.indexOf(256)]);
await browser.close();
for (const f of ["device.icns", "device.ico", "device-256.png", "device-macos-512.png"]) console.log(`crates/dstudio-app/icons/${f}`);

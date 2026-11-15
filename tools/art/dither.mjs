// Writes the ordered-dither material used behind the public artwork and the website.
//
//   node tools/art/dither.mjs            # every texture listed in TEXTURES below
//
// No dependencies: the field is computed here, quantised with an 8x8 Bayer matrix to four
// colours from the app's tokens, and written as a palette PNG with node:zlib. Every texel is
// meant to be shown as a crisp square (CSS `image-rendering: pixelated`) at a whole number of
// pixels, so the pattern stays an honest dither instead of being smoothed into a gradient.
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

// Ground, a deep blue between ground and accent, the accent, and the accent's light text tone.
export const PALETTE = [
  [0x0b, 0x0c, 0x0e],
  [0x15, 0x24, 0x5f],
  [0x27, 0x52, 0xf5],
  [0x8e, 0xa6, 0xff],
];

// Classic recursive Bayer matrix, thresholds in (0, 1).
export function bayer(n = 8) {
  let m = [[0]];
  for (let size = 1; size < n; size *= 2) {
    const next = [];
    for (let y = 0; y < size * 2; y++) {
      next.push([]);
      for (let x = 0; x < size * 2; x++) {
        const q = 4 * m[y % size][x % size];
        next[y].push(q + [0, 2, 3, 1][(y >= size ? 2 : 0) + (x >= size ? 1 : 0)]);
      }
    }
    m = next;
  }
  return m.map((row) => row.map((v) => (v + 0.5) / (n * n)));
}

const smooth = (a, b, x) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
};

// Tone in [0, 1] at a point, in texture-relative coordinates (0..1 across, 0..1 down).
// A broad bloom strongest at the lower right, a quiet band along the bottom edge, and one
// faint sine cycle (the mark) sweeping through the bloom. Calm reading space top left.
function tone(u, v, aspect, { sine, gain = 1 }) {
  const dx = (1.04 - u) * aspect;
  const dy = 1.08 - v;
  const r = Math.hypot(dx, dy) / Math.hypot(aspect, 1);
  let t = 0.78 * (1 - smooth(0.0, 0.62, r)) ** 1.35;
  t += 0.22 * smooth(0.55, 1.0, v) * smooth(0.0, 0.9, u);
  if (sine) {
    const start = 0.3;
    const p = (u - start) / (1 - start);
    if (p > 0) {
      const ys = 0.66 - 0.16 * Math.sin(p * Math.PI * 2);
      const d = Math.abs(v - ys) * aspect;
      const fade = smooth(0, 0.35, p);
      t += fade * (0.42 * Math.exp(-((d / 0.016) ** 2)) + 0.2 * Math.exp(-((d / 0.05) ** 2)));
    }
  }
  return Math.min(1, t * gain);
}

export function ditherField(width, height, opts = {}) {
  const m = bayer(8);
  const levels = PALETTE.length - 1;
  const out = new Uint8Array(width * height);
  const aspect = width / height;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const n = tone((x + 0.5) / width, (y + 0.5) / height, aspect, opts) * levels;
      const base = Math.floor(n);
      const level = Math.min(levels, base + (n - base > m[y & 7][x & 7] ? 1 : 0));
      out[y * width + x] = level;
    }
  }
  return out;
}

function crc32(buf) {
  let c = ~0;
  for (const b of buf) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

export function palettePng(width, height, indices) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 3; // palette
  const raw = Buffer.alloc((width + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width + 1)] = 0; // no filter: palette data compresses best unfiltered
    raw.set(indices.subarray(y * width, (y + 1) * width), y * (width + 1) + 1);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("PLTE", Buffer.from(PALETTE.flat())),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// The README art is shown at 3 CSS px per texel (6 device px in the 2x renders), which
// survives GitHub scaling the images down. The website texture is shown at 2 CSS px per
// texel on wide screens and 1 on phones; copy it to devicedaw.github.io/assets/dither.png.
const TEXTURES = [
  { file: "docs/art/textures/dither.png", width: 480, height: 360, sine: true },
  { file: "docs/art/textures/dither-site.png", width: 1280, height: 720, sine: false, gain: 0.42 },
];

if (import.meta.url === `file://${process.argv[1]}`) {
  for (const t of TEXTURES) {
    const png = palettePng(t.width, t.height, ditherField(t.width, t.height, t));
    const path = join(root, t.file);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, png);
    console.log(`${t.file} ${t.width}x${t.height} ${png.length} bytes`);
  }
}

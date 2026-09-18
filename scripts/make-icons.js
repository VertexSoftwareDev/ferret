/*
 * Generates Ferret's application icons.
 *
 * The mark is drawn from maths rather than shipped as a binary blob, so the
 * icon is reviewable in a diff and can be recoloured by editing two constants.
 * Run with: node scripts/make-icons.js
 */

'use strict';

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const OUT = path.join(__dirname, '..', 'icons');

/** Indigo, matching --accent in the UI. */
const ACCENT = [99, 102, 241];
const GLASS = [255, 255, 255];
/** Supersampling factor; 4x is plenty to hide the stair-steps. */
const SS = 4;

/** Signed distance from a point to a circle outline. */
function ringDistance(x, y, cx, cy, radius) {
  return Math.abs(Math.hypot(x - cx, y - cy) - radius);
}

/** Signed distance from a point to a line segment. */
function segmentDistance(x, y, x1, y1, x2, y2) {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const lengthSq = dx * dx + dy * dy;
  const t = lengthSq === 0 ? 0 : Math.max(0, Math.min(1, ((x - x1) * dx + (y - y1) * dy) / lengthSq));
  return Math.hypot(x - (x1 + t * dx), y - (y1 + t * dy));
}

/** Distance to a rounded rectangle's interior (negative inside). */
function roundedRect(x, y, size, radius) {
  const half = size / 2;
  const qx = Math.abs(x - half) - (half - radius);
  const qy = Math.abs(y - half) - (half - radius);
  const outside = Math.hypot(Math.max(qx, 0), Math.max(qy, 0));
  return outside + Math.min(Math.max(qx, qy), 0) - radius;
}

/** Render the icon at `size` px as raw RGBA. */
function draw(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const u = size / 24; // the mark is designed on a 24-unit grid

  const glassCx = 10.2 * u;
  const glassCy = 10.2 * u;
  const glassR = 5.6 * u;
  const stroke = 2.1 * u;
  const handle = [14.4 * u, 14.4 * u, 19.2 * u, 19.2 * u];

  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let bgHits = 0;
      let fgHits = 0;

      // Supersample: count how many sub-samples land on background and mark.
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const x = px + (sx + 0.5) / SS;
          const y = py + (sy + 0.5) / SS;

          if (roundedRect(x, y, size, 5.2 * u) <= 0) bgHits++;

          const onRing = ringDistance(x, y, glassCx, glassCy, glassR) <= stroke / 2;
          const onHandle = segmentDistance(x, y, ...handle) <= stroke / 2;
          if (onRing || onHandle) fgHits++;
        }
      }

      const total = SS * SS;
      const bgAlpha = bgHits / total;
      const fgAlpha = fgHits / total;
      const offset = (py * size + px) * 4;

      // Composite the white mark over the indigo tile.
      const alpha = Math.max(bgAlpha, fgAlpha);
      if (alpha === 0) continue;

      const mix = fgAlpha;
      for (let c = 0; c < 3; c++) {
        pixels[offset + c] = Math.round(ACCENT[c] * (1 - mix) + GLASS[c] * mix);
      }
      pixels[offset + 3] = Math.round(alpha * 255);
    }
  }

  return pixels;
}

function crc32(buf) {
  let crc = ~0;
  for (const byte of buf) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ~crc >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

function encodePng(size, pixels) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8; // bit depth
  header[9] = 6; // colour type: RGBA
  // 10..12 stay zero: deflate, adaptive filtering, no interlace.

  // Each scanline is prefixed with its filter type; 0 means "none".
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    pixels.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/** Wrap PNGs in an ICO container; Vista and later accept PNG-compressed entries. */
function encodeIco(images) {
  const dir = Buffer.alloc(6);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2); // type: icon
  dir.writeUInt16LE(images.length, 4);

  let offset = 6 + images.length * 16;
  const entries = [];
  for (const { size, png } of images) {
    const entry = Buffer.alloc(16);
    entry[0] = size >= 256 ? 0 : size; // 0 means 256
    entry[1] = size >= 256 ? 0 : size;
    entry[2] = 0; // palette size
    entry[3] = 0;
    entry.writeUInt16LE(1, 4); // colour planes
    entry.writeUInt16LE(32, 6); // bits per pixel
    entry.writeUInt32LE(png.length, 8);
    entry.writeUInt32LE(offset, 12);
    entries.push(entry);
    offset += png.length;
  }

  return Buffer.concat([dir, ...entries, ...images.map((i) => i.png)]);
}

fs.mkdirSync(OUT, { recursive: true });

const sizes = [16, 32, 48, 64, 128, 256];
const rendered = sizes.map((size) => ({ size, png: encodePng(size, draw(size)) }));

for (const { size, png } of rendered) {
  fs.writeFileSync(path.join(OUT, `${size}x${size}.png`), png);
}
// Tauri looks for this exact name for the 128px @2x variant.
fs.copyFileSync(path.join(OUT, '256x256.png'), path.join(OUT, '128x128@2x.png'));
fs.writeFileSync(path.join(OUT, 'icon.png'), rendered.at(-1).png);
fs.writeFileSync(path.join(OUT, 'icon.ico'), encodeIco(rendered));

console.log(`ikonlar yazildi: ${OUT}`);

// Draws the application icon and writes the files the Tauri bundle expects:
// src-tauri/icons/{32x32.png,128x128.png,128x128@2x.png,icon.ico}, and the
// picture for the menu bar of macOS, tray-template.png: the letter alone, in
// black, which the system colours for a light or a dark bar.
//
// The icon is described here in code, so it needs no source image and no
// packages: PNG is written with zlib from Node. The ICO holds ordinary
// bitmaps, not PNG frames: NSIS copies those into the installer, and a PNG
// frame leaves the setup file with the default icon.
//
//   node scripts/make-icons.mjs

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "src-tauri", "icons");

// Samples per pixel side: 8×8 samples smooth the edges at every size.
const SAMPLES = 8;

// The plate: a rounded square with a diagonal gradient.
const PLATE_INSET = 0.04;
const PLATE_RADIUS = 0.22;
const TOP_LEFT = [0x3b, 0x82, 0xf6];
const BOTTOM_RIGHT = [0x7c, 0x3a, 0xed];

// The letter "P" out of square pixels.
const GLYPH = ["XXXX.", "X...X", "X...X", "XXXX.", "X....", "X...."];
const GLYPH_HEIGHT = 0.58;
const CELL_FILL = 0.88;
const WHITE = [0xff, 0xff, 0xff];

// The picture for the menu bar: the letter fills most of the height.
const TEMPLATE_SIZE = 72;
const TEMPLATE_HEIGHT = 0.8;

const glyphRows = GLYPH.length;
const glyphColumns = GLYPH[0].length;

/** Where the letter of this height sits in the unit square. */
function placed(height) {
  const cell = height / glyphRows;
  return { cell, left: (1 - cell * glyphColumns) / 2 + cell * 0.25, top: (1 - height) / 2 };
}

const ICON_GLYPH = placed(GLYPH_HEIGHT);
const TEMPLATE_GLYPH = placed(TEMPLATE_HEIGHT);

/** Whether the point of the unit square lies on the rounded plate. */
function onPlate(x, y) {
  const low = PLATE_INSET;
  const high = 1 - PLATE_INSET;
  if (x < low || x > high || y < low || y > high) {
    return false;
  }
  const cx = Math.min(Math.max(x, low + PLATE_RADIUS), high - PLATE_RADIUS);
  const cy = Math.min(Math.max(y, low + PLATE_RADIUS), high - PLATE_RADIUS);
  return (x - cx) ** 2 + (y - cy) ** 2 <= PLATE_RADIUS ** 2;
}

/** Whether the point lies on one of the squares of the letter. */
function onGlyph(x, y, glyph) {
  const column = Math.floor((x - glyph.left) / glyph.cell);
  const row = Math.floor((y - glyph.top) / glyph.cell);
  if (row < 0 || row >= glyphRows || column < 0 || column >= glyphColumns) {
    return false;
  }
  if (GLYPH[row][column] !== "X") {
    return false;
  }
  const inCellX = (x - glyph.left) / glyph.cell - column;
  const inCellY = (y - glyph.top) / glyph.cell - row;
  const edge = (1 - CELL_FILL) / 2;
  return inCellX >= edge && inCellX <= 1 - edge && inCellY >= edge && inCellY <= 1 - edge;
}

/** RGBA pixels of the icon, not premultiplied. */
function draw(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const total = SAMPLES * SAMPLES;
  for (let py = 0; py < size; py += 1) {
    for (let px = 0; px < size; px += 1) {
      let covered = 0;
      const sum = [0, 0, 0];
      for (let sy = 0; sy < SAMPLES; sy += 1) {
        for (let sx = 0; sx < SAMPLES; sx += 1) {
          const x = (px + (sx + 0.5) / SAMPLES) / size;
          const y = (py + (sy + 0.5) / SAMPLES) / size;
          if (!onPlate(x, y)) {
            continue;
          }
          covered += 1;
          const along = (x + y) / 2;
          const white = onGlyph(x, y, ICON_GLYPH);
          for (let channel = 0; channel < 3; channel += 1) {
            sum[channel] += white
              ? WHITE[channel]
              : TOP_LEFT[channel] + (BOTTOM_RIGHT[channel] - TOP_LEFT[channel]) * along;
          }
        }
      }
      const at = (py * size + px) * 4;
      if (covered > 0) {
        pixels[at] = Math.round(sum[0] / covered);
        pixels[at + 1] = Math.round(sum[1] / covered);
        pixels[at + 2] = Math.round(sum[2] / covered);
        pixels[at + 3] = Math.round((covered / total) * 255);
      }
    }
  }
  return pixels;
}

/** RGBA pixels of the letter alone: black, as opaque as the letter covers the pixel. */
function drawTemplate(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const total = SAMPLES * SAMPLES;
  for (let py = 0; py < size; py += 1) {
    for (let px = 0; px < size; px += 1) {
      let covered = 0;
      for (let sy = 0; sy < SAMPLES; sy += 1) {
        for (let sx = 0; sx < SAMPLES; sx += 1) {
          const x = (px + (sx + 0.5) / SAMPLES) / size;
          const y = (py + (sy + 0.5) / SAMPLES) / size;
          if (onGlyph(x, y, TEMPLATE_GLYPH)) {
            covered += 1;
          }
        }
      }
      pixels[(py * size + px) * 4 + 3] = Math.round((covered / total) * 255);
    }
  }
  return pixels;
}

const crcTable = new Uint32Array(256);
for (let n = 0; n < 256; n += 1) {
  let c = n;
  for (let k = 0; k < 8; k += 1) {
    c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  }
  crcTable[n] = c >>> 0;
}

function crc32(bytes) {
  let c = 0xffffffff;
  for (const byte of bytes) {
    c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const sum = Buffer.alloc(4);
  sum.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, sum]);
}

/** An 8-bit RGBA PNG of square `pixels`. */
function png(size, pixels = draw(size)) {
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y += 1) {
    // Every row starts with its filter type; 0 means the bytes are stored as they are.
    pixels.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8; // bits per channel
  header[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** One icon size as a 32-bit bitmap: header, bottom-up BGRA and the mask. */
function bmpFrame(size) {
  const pixels = draw(size);
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  // The height counts the colour rows and the mask rows.
  header.writeInt32LE(size * 2, 8);
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  const colours = Buffer.alloc(size * size * 4);
  const maskStride = Math.ceil(size / 32) * 4;
  const mask = Buffer.alloc(maskStride * size);
  for (let y = 0; y < size; y += 1) {
    const source = (size - 1 - y) * size;
    for (let x = 0; x < size; x += 1) {
      const from = (source + x) * 4;
      const to = (y * size + x) * 4;
      // Windows stores a 32-bit icon with the colour already multiplied by
      // the alpha. Straight colour leaves a fringe on the rounded edge.
      const alpha = pixels[from + 3];
      const scale = alpha / 255;
      colours[to] = Math.round(pixels[from + 2] * scale);
      colours[to + 1] = Math.round(pixels[from + 1] * scale);
      colours[to + 2] = Math.round(pixels[from] * scale);
      colours[to + 3] = alpha;
      // A clear pixel is transparent even for a reader that ignores the alpha.
      if (pixels[from + 3] === 0) {
        mask[y * maskStride + (x >> 3)] |= 0x80 >> (x & 7);
      }
    }
  }
  header.writeUInt32LE(colours.length + mask.length, 20);
  return Buffer.concat([header, colours, mask]);
}

/** An ICO file whose frames are bitmaps. */
function ico(sizes) {
  const frames = sizes.map((size) => ({ size, data: bmpFrame(size) }));
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(frames.length, 4);
  const entries = [];
  let offset = 6 + 16 * frames.length;
  for (const frame of frames) {
    const entry = Buffer.alloc(16);
    // A side of 256 pixels is written as 0.
    entry[0] = frame.size === 256 ? 0 : frame.size;
    entry[1] = frame.size === 256 ? 0 : frame.size;
    entry.writeUInt16LE(1, 4); // colour planes
    entry.writeUInt16LE(32, 6); // bits per pixel
    entry.writeUInt32LE(frame.data.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += frame.data.length;
    entries.push(entry);
  }
  return Buffer.concat([header, ...entries, ...frames.map((frame) => frame.data)]);
}

mkdirSync(out, { recursive: true });
const files = {
  "32x32.png": png(32),
  "128x128.png": png(128),
  "128x128@2x.png": png(256),
  "icon.ico": ico([32, 16, 24, 48, 64, 256]),
  "tray-template.png": png(TEMPLATE_SIZE, drawTemplate(TEMPLATE_SIZE)),
};
for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(out, name), data);
  console.log(`${name}: ${data.length} bytes`);
}

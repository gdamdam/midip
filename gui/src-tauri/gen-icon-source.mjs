// Generates a 1024×1024 source icon for midip-gui: a 4×4 step-sequencer grid,
// some cells lit ember, on a rounded charcoal panel. Feed to `tauri icon` to
// produce the platform icon set. Pure Node (zlib) PNG encoder — no deps.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const SIZE = 1024;
const BG = [29, 32, 33]; // #1D2021 charcoal
const DIM = [80, 73, 69]; // #504945 unlit cell
const EMBER = [254, 128, 25]; // #FE8019 lit cell

// Lit pattern (matches the chosen concept).
const LIT = [
  [1, 0, 1, 0],
  [0, 1, 0, 0],
  [1, 0, 0, 1],
  [0, 0, 1, 0],
];

// RGBA canvas.
const px = new Uint8Array(SIZE * SIZE * 4); // all zero = transparent

function inRoundedRect(x, y, rx, ry, rw, rh, r) {
  if (x < rx || x >= rx + rw || y < ry || y >= ry + rh) return false;
  const dxl = rx + r,
    dxr = rx + rw - 1 - r,
    dyt = ry + r,
    dyb = ry + rh - 1 - r;
  let cx = x,
    cy = y;
  if (x < dxl) cx = dxl;
  else if (x > dxr) cx = dxr;
  if (y < dyt) cy = dyt;
  else if (y > dyb) cy = dyb;
  const dx = x - cx,
    dy = y - cy;
  return dx * dx + dy * dy <= r * r;
}

function fillRounded(rx, ry, rw, rh, r, color) {
  const x0 = Math.max(0, rx),
    y0 = Math.max(0, ry);
  const x1 = Math.min(SIZE, rx + rw),
    y1 = Math.min(SIZE, ry + rh);
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      if (inRoundedRect(x, y, rx, ry, rw, rh, r)) {
        const o = (y * SIZE + x) * 4;
        px[o] = color[0];
        px[o + 1] = color[1];
        px[o + 2] = color[2];
        px[o + 3] = 255;
      }
    }
  }
}

// Rounded charcoal panel (transparent margin outside).
const margin = 40;
fillRounded(margin, margin, SIZE - 2 * margin, SIZE - 2 * margin, 180, BG);

// 4×4 grid centred inside the panel.
const pad = 150; // inner padding from panel edge
const area = SIZE - 2 * pad;
const gap = 40;
const cell = Math.floor((area - 3 * gap) / 4);
const start = pad;
for (let row = 0; row < 4; row++) {
  for (let col = 0; col < 4; col++) {
    const x = start + col * (cell + gap);
    const y = start + row * (cell + gap);
    fillRounded(x, y, cell, cell, 26, LIT[row][col] ? EMBER : DIM);
  }
}

// --- PNG encode ---
function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return (~c) >>> 0;
}
function chunk(type, data) {
  const t = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([t, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}
const stride = SIZE * 4 + 1;
const raw = Buffer.alloc(stride * SIZE);
for (let y = 0; y < SIZE; y++) {
  raw[y * stride] = 0;
  px.subarray(y * SIZE * 4, (y + 1) * SIZE * 4).forEach((v, i) => {
    raw[y * stride + 1 + i] = v;
  });
}
const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8;
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  sig,
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw)),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync(new URL("./icon-source.png", import.meta.url), png);
console.log("wrote icon-source.png (1024x1024)");

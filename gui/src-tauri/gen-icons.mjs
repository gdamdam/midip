// Generates simple solid Ember-orange PNG icons for the Tauri bundle.
// Pure Node (zlib) PNG encoder — no external deps. Run: node gen-icons.mjs
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";

mkdirSync(new URL("./icons/", import.meta.url), { recursive: true });

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
// Ember orange #FE8019 on dark #1D2021 with a simple centered square motif.
function png(size) {
  const bg = [29, 32, 33, 255];
  const fg = [254, 128, 25, 255];
  const stride = size * 4 + 1;
  const raw = Buffer.alloc(stride * size);
  const m = Math.floor(size * 0.22);
  for (let y = 0; y < size; y++) {
    raw[y * stride] = 0; // filter type 0
    for (let x = 0; x < size; x++) {
      const inner = x >= m && x < size - m && y >= m && y < size - m;
      const c = inner ? fg : bg;
      const o = y * stride + 1 + x * 4;
      raw[o] = c[0]; raw[o + 1] = c[1]; raw[o + 2] = c[2]; raw[o + 3] = c[3];
    }
  }
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;   // bit depth
  ihdr[9] = 6;   // RGBA
  return Buffer.concat([
    sig,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
const out = new URL("./icons/", import.meta.url);
const files = {
  "32x32.png": 32,
  "128x128.png": 128,
  "128x128@2x.png": 256,
  "icon.png": 512,
};
for (const [name, size] of Object.entries(files)) {
  writeFileSync(new URL(name, out), png(size));
  console.log("wrote icons/" + name);
}

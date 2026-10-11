// Writes the fixtures image-edit-shots.mjs needs into one directory: images.pdf (one page, a heading, two
// pictures: a 240 x 160 pt block of colour and a 120 x 120 pt checkerboard) and two PNG files to replace with
// and to add (logo.png 80 x 50, photo.png 120 x 80). Everything is generated, no file of the user's.
//   node scripts/make-image-edit-sample.mjs "$env:TEMP\inkwell-image-edit"
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const dir = process.argv[2];
if (!dir) {
  console.error("usage: node scripts/make-image-edit-sample.mjs <dir>");
  process.exit(1);
}
fs.mkdirSync(dir, { recursive: true });

/** w x h RGB samples from f(x, y) -> [r, g, b]. */
const raster = (w, h, f) => {
  const px = Buffer.alloc(w * h * 3);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) px.set(f(x, y), (y * w + x) * 3);
  return px;
};

// ---- PNG ----
const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (b) => {
  let c = 0xffffffff;
  for (const v of b) c = crcTable[(c ^ v) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const t = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
};
const png = (w, h, px) => {
  const rows = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) px.copy(rows, y * (w * 3 + 1) + 1, y * w * 3, (y + 1) * w * 3);
  const head = Buffer.alloc(13);
  head.writeUInt32BE(w, 0);
  head.writeUInt32BE(h, 4);
  head.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", head),
    chunk("IDAT", zlib.deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
};

fs.writeFileSync(
  path.join(dir, "logo.png"),
  png(80, 50, raster(80, 50, (x, y) => (y >= 20 && y < 30 ? [255, 255, 255] : [16, 130, 140]))),
);
fs.writeFileSync(
  path.join(dir, "photo.png"),
  png(120, 80, raster(120, 80, (x, y) => [Math.round((x / 119) * 255), Math.round((y / 79) * 255), 160])),
);

// ---- the PDF ----
const A = { w: 120, h: 80, px: raster(120, 80, (x, y) => [[220, 60, 60], [60, 160, 90], [60, 100, 200]][Math.floor(x / 40)].map((v) => v - (y > 40 ? 40 : 0))) };
const B = { w: 8, h: 8, px: raster(8, 8, (x, y) => ((x + y) % 2 ? [30, 30, 30] : [240, 200, 60])) };
const imageObj = (i) => {
  const data = zlib.deflateSync(i.px);
  return [
    `<< /Type /XObject /Subtype /Image /Width ${i.w} /Height ${i.h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length ${data.length} >>\nstream\n`,
    data,
    "\nendstream",
  ];
};
// page 612 x 792: A is 240 x 160 with its top left at (72, 120); B is 120 x 120 with its top left at (372, 400)
const content = Buffer.from(
  [
    "BT /F1 20 Tf 72 700 Td (Two pictures to edit) Tj ET",
    "q 240 0 0 160 72 512 cm /ImA Do Q",
    "q 120 0 0 120 372 272 cm /ImB Do Q",
    "BT /F1 12 Tf 72 240 Td (The block above and the checkerboard are separate images.) Tj ET",
    "",
  ].join("\n"),
);
const objects = [
  "<< /Type /Catalog /Pages 2 0 R >>",
  "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
  "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /ImA 6 0 R /ImB 7 0 R >> >> >>",
  [`<< /Length ${content.length} >>\nstream\n`, content, "\nendstream"],
  "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  imageObj(A),
  imageObj(B),
];
const parts = [Buffer.from("%PDF-1.4\n")];
const offsets = [];
let at = parts[0].length;
objects.forEach((o, i) => {
  offsets.push(at);
  const b = Buffer.concat(
    [Buffer.from(`${i + 1} 0 obj\n`), ...(Array.isArray(o) ? o.map((p) => (typeof p === "string" ? Buffer.from(p, "latin1") : p)) : [Buffer.from(o)]), Buffer.from("\nendobj\n")],
  );
  parts.push(b);
  at += b.length;
});
let xref = `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
for (const o of offsets) xref += `${String(o).padStart(10, "0")} 00000 n \n`;
xref += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${at}\n%%EOF\n`;
parts.push(Buffer.from(xref));
fs.writeFileSync(path.join(dir, "images.pdf"), Buffer.concat(parts));
console.log("wrote", dir);

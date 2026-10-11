// Writes a one-page PDF whose only picture is six flat blocks of pure colour (red, green, blue / cyan, magenta,
// yellow), for page-dark-shots.mjs: on a dark page the picture must come back with exactly these colours.
//   node scripts/make-rgb-photo.mjs "$env:TEMP\inkwell-page-dark\in\rgb-photo.pdf"
import fs from "node:fs";
import zlib from "node:zlib";

const out = process.argv[2];
if (!out) {
  console.error("usage: node scripts/make-rgb-photo.mjs <out.pdf>");
  process.exit(1);
}

const W = 60, H = 40;
const colours = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [0, 255, 255], [255, 0, 255], [255, 255, 0]];
const px = Buffer.alloc(W * H * 3);
for (let y = 0; y < H; y++)
  for (let x = 0; x < W; x++) px.set(colours[(y < H / 2 ? 0 : 3) + Math.floor(x / (W / 3))], (y * W + x) * 3);
const image = zlib.deflateSync(px);

// the picture is 300 x 200 pt with its lower left corner at (100, 500) on a 595 x 842 page
const content = Buffer.from("BT /F1 14 Tf 100 740 Td (Six flat colours, for the dark pages check) Tj ET\nq 300 0 0 200 100 500 cm /Im1 Do Q\n");
const objects = [
  "<< /Type /Catalog /Pages 2 0 R >>",
  "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
  "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> >>",
  [`<< /Length ${content.length} >>\nstream\n`, content, "\nendstream"],
  "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  [
    `<< /Type /XObject /Subtype /Image /Width ${W} /Height ${H} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length ${image.length} >>\nstream\n`,
    image,
    "\nendstream",
  ],
];

const parts = [Buffer.from("%PDF-1.7\n")];
const offsets = [];
let size = parts[0].length;
objects.forEach((o, i) => {
  offsets.push(size);
  const chunks = [Buffer.from(`${i + 1} 0 obj\n`), ...(Array.isArray(o) ? o.map((c) => (Buffer.isBuffer(c) ? c : Buffer.from(c))) : [Buffer.from(o)]), Buffer.from("\nendobj\n")];
  for (const c of chunks) {
    parts.push(c);
    size += c.length;
  }
});
const xref = [`xref\n0 ${objects.length + 1}\n`, "0000000000 65535 f \n", ...offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`)].join("");
parts.push(Buffer.from(`${xref}trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${size}\n%%EOF\n`));
fs.writeFileSync(out, Buffer.concat(parts));
console.log("wrote", out);

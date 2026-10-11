// Writes a PDF of N empty A4 pages, for checking the toolbar's page-number box at 1, 91 and 1500 pages.
//   node scripts/make-blank-pages.mjs "$env:TEMP\inkwell-page-dark\in\pages-91.pdf" 91
import fs from "node:fs";

const [out, n] = [process.argv[2], Number(process.argv[3])];
if (!out || !(n >= 1)) {
  console.error("usage: node scripts/make-blank-pages.mjs <out.pdf> <pages>");
  process.exit(1);
}

// 1 catalog, 2 pages, 3 font, 4 empty stream, then one page object each
const objects = [
  "<< /Type /Catalog /Pages 2 0 R >>",
  `<< /Type /Pages /Kids [${Array.from({ length: n }, (_, i) => `${5 + i} 0 R`).join(" ")}] /Count ${n} >>`,
  "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  "<< /Length 0 >>\nstream\n\nendstream",
  ...Array.from({ length: n }, () => "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 3 0 R >> >> >>"),
];
let body = "%PDF-1.7\n";
const offsets = [];
objects.forEach((o, i) => {
  offsets.push(body.length);
  body += `${i + 1} 0 obj\n${o}\nendobj\n`;
});
const xref = `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n${offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
body += `${xref}trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${body.length}\n%%EOF\n`;
fs.writeFileSync(out, body, "latin1");
console.log("wrote", out, n, "pages");

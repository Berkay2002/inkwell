// Writes a 2-page sample lease with a signatures page (form lines, a checkbox, an empty "Landlord" signature
// field) for the Fill & Sign screenshots. Plain PDF, Helvetica, no libraries; all names are made up.
//   node scripts/sign-sample.mjs "<dir>/Lease agreement.pdf"
import fs from "node:fs";

const esc = (t) => t.replace(/[\()]/g, (c) => `\${c}`);
const text = (x, y, size, t, bold = false) => `BT /${bold ? "F2" : "F1"} ${size} Tf ${x} ${y} Td (${esc(t)}) Tj ET\n`;
const line = (x0, x1, y) => `0.6 w 0.35 0.35 0.35 RG ${x0} ${y} m ${x1} ${y} l S\n`;

const terms = [
  "This Lease is made between Maple Property Management AB (the Landlord)",
  "and the Tenant named on the signatures page.",
  "The monthly rent is 12 400 SEK, due on the 25th day of each month.",
  "The Tenant shall keep the apartment clean and in good repair.",
  "Either party may end this Lease with three months' written notice.",
];
const pages = [
  text(64, 700, 26, "Residential Lease Agreement", true) + terms.map((l, i) => text(64, 650 - 20 * i, 12, l)).join(""),
  [
    text(64, 720, 16, "7. SIGNATURES", true),
    text(64, 690, 12, "By signing, the parties accept every term of this agreement."),
    text(64, 640, 12, "Tenant name:"),
    line(150, 400, 637),
    text(64, 610, 12, "Date:"),
    line(150, 400, 607),
    `0.8 w 0.35 0.35 0.35 RG 64 570 12 12 re S\n`,
    text(84, 572, 12, "I have received the keys"),
    line(64, 280, 480),
    text(64, 464, 10, "Landlord"),
    line(332, 548, 480),
    text(332, 464, 10, "Tenant"),
    line(64, 280, 380),
    text(64, 364, 10, "Witness"),
  ].join(""),
];

const objs = [];
const add = (s) => objs.push(s) && objs.length;
const f1 = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
const f2 = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>");
// fonts, then per page a stream and a page, then the field, the pages node and the catalog
const fieldId = 2 + 2 * pages.length + 1;
const pagesId = fieldId + 1;
const kids = [];
pages.forEach((c, i) => {
  const s = add(`<< /Length ${c.length} >>\nstream\n${c}endstream`);
  const annots = i === 1 ? ` /Annots [${fieldId} 0 R]` : "";
  kids.push(add(`<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 612 792] /Contents ${s} 0 R /Resources << /Font << /F1 ${f1} 0 R /F2 ${f2} 0 R >> >>${annots} >>`));
});
if (add(`<< /Type /Annot /Subtype /Widget /FT /Sig /T (Landlord) /F 4 /Rect [64 482 280 540] /P ${kids[1]} 0 R >>`) !== fieldId) throw new Error("field id");
if (add(`<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`) !== pagesId) throw new Error("pages id");
const cat = add(`<< /Type /Catalog /Pages ${pagesId} 0 R /AcroForm << /Fields [${fieldId} 0 R] >> >>`);

let out = "%PDF-1.7\n";
const offs = [];
objs.forEach((o, i) => {
  offs.push(out.length);
  out += `${i + 1} 0 obj\n${o}\nendobj\n`;
});
const xref = out.length;
out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offs.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
out += `trailer\n<< /Size ${objs.length + 1} /Root ${cat} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
fs.writeFileSync(process.argv[2], out, "latin1");

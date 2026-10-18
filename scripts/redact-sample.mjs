// Writes a 9-page sample lease (plain PDF, Helvetica, no libraries) with a personnummer, e-mails, phones, a card
// number and an IBAN, for the Redact screenshots. All values are made up.
//   node scripts/redact-sample.mjs "<dir>/Lease agreement.pdf"
import fs from "node:fs";

const esc = (t) => t.replace(/[\\()]/g, (c) => `\\${c}`);
const text = (x, y, size, t, bold = false) => `BT /${bold ? "F2" : "F1"} ${size} Tf ${x} ${y} Td (${esc(t)}) Tj ET\n`;

const body = [
  "The Tenant shall comply with all applicable laws, ordinances and rules,",
  "and shall use the Premises only for the purpose described in this Lease.",
  "Notices under this clause shall be given in writing to the other party.",
];
const pages = [
  [text(64, 660, 30, "Residential", true) + text(64, 622, 30, "Lease Agreement", true) + text(64, 560, 12, "Maple Property Management AB and the Tenant named below")],
  [
    text(64, 720, 20, "2. PARTIES", true),
    text(64, 690, 12, "This Residential Lease Agreement is made on 1 October 2026 between Maple Property"),
    text(64, 674, 12, "Management AB (the Landlord) and the Tenant named below."),
    text(64, 630, 14, "Tenant", true),
    text(64, 600, 12, "Name:", true) + text(190, 600, 12, "Alex Johnson"),
    text(64, 578, 12, "Personnummer:", true) + text(190, 578, 12, "121212-1212"),
    text(64, 556, 12, "E-mail:", true) + text(190, 556, 12, "alex.johnson@example.com"),
    text(64, 534, 12, "Phone:", true) + text(190, 534, 12, "+46 70 123 45 67"),
    text(64, 480, 16, "2.3 Rent", true),
    text(64, 456, 12, "The rent for the Premises is 12 400 SEK per month, which shall be paid"),
    text(64, 440, 12, "on the 25th day of each month to the Landlord's bank account."),
    text(64, 416, 12, "Tenant e-mail: alex.johnson@example.com"),
  ].join(""),
  ...[3, 4].map((n) => [text(64, 720, 16, `${n}. TERM AND USE`, true), ...body.map((l, i) => text(64, 690 - 18 * i, 12, l))].join("")),
  [
    text(64, 720, 16, "5. NOTICES", true),
    text(64, 690, 12, "Notices shall be sent by e-mail to alex.johnson@example.com or by post."),
    text(64, 672, 12, "Questions about this Lease: contact the Landlord on +46 8 555 010 20."),
  ].join(""),
  ...[6, 7].map((n) => [text(64, 720, 16, `${n}. GENERAL`, true), ...body.map((l, i) => text(64, 690 - 18 * i, 12, l))].join("")),
  [
    text(64, 720, 16, "8. PAYMENT", true),
    text(64, 690, 12, "Pay by card 4111 1111 1111 1111 or by transfer to IBAN SE45 5000 0000 0583 9825 7466."),
  ].join(""),
  [text(64, 720, 16, "9. CONTACT", true), text(64, 690, 12, "Contact the tenant at alex.johnson@example.com")].join(""),
];

const objs = [];
const add = (s) => objs.push(s) && objs.length;
const f1 = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
const f2 = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>");
const pagesId = 2 + 2 * pages.length + 1; // added after the fonts and the page objects
const kids = [];
for (const c of pages.map((p) => [p].flat().join(""))) {
  const s = add(`<< /Length ${c.length} >>\nstream\n${c}endstream`);
  kids.push(add(`<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 612 792] /Contents ${s} 0 R /Resources << /Font << /F1 ${f1} 0 R /F2 ${f2} 0 R >> >> >>`));
}
if (add(`<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`) !== pagesId) throw new Error("pages id");
const cat = add(`<< /Type /Catalog /Pages ${pagesId} 0 R >>`);

let out = "%PDF-1.4\n";
const offs = [];
objs.forEach((o, i) => {
  offs.push(out.length);
  out += `${i + 1} 0 obj\n${o}\nendobj\n`;
});
const xref = out.length;
out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offs.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
out += `trailer\n<< /Size ${objs.length + 1} /Root ${cat} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
fs.writeFileSync(process.argv[2], out, "latin1");

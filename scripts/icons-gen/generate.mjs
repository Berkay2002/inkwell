// Generates the Inkwell icons that Fluent has no glyph for, in the Fluent style, with Codex's built-in image tool.
//   node scripts/icons-gen/generate.mjs <outDir> <name>...      (needs the `codex` CLI, signed in)
// For each name one PNG with the Regular (outline) icon on the left and the Filled (solid) one on the right is written
// to <outDir>/<name>.png, using scripts/icons-gen/reference.png (some Fluent icons, Regular above Filled) as the style
// reference. Then trace them: node scripts/icons-gen/trace.cjs <outDir> src/assets/icons/inkwell <name>...
// Up to four run at a time; each takes about a minute. The prompts sent contain only the reference image (Fluent
// icons, MIT) and the concept lines below.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const ref = path.join(here, "reference.png");

/** The concept of each glyph, as it is given to the image model. */
export const concepts = {
  redact: "a document page with one short text line near the top and one wide, thick redaction bar below it that reaches across most of the page width (in the Filled version the page is solid black, the text line and the redaction bar are plain white shapes knocked out, with no frame or outline around the bar)",
  bates: "a document page with a number sign (#) in the middle of it, as for Bates page numbering",
  pdfa: "a document page with a small archive storage box (a box with a lid and a small label slot) overlapping its bottom-right corner, like a document with a badge",
  ocr: "four corner scan brackets (like a viewfinder / scan frame) with a capital letter A in the centre, meaning recognise text in a scan",
  compress: "a tall document page that fills the full height of the icon, with one short arrow on each side, close to the page, pointing inward at it; the whole composition is roughly square, the page is the dominant shape",
  measure: "a diagonal ruler running from bottom-left to top-right with evenly spaced tick marks along one long edge",
  batch: "two stacked document pages (one behind the other, offset) with a small play triangle on the front page, meaning run on many files",
  prepress: "a print registration target mark: a circle with a crosshair, the cross lines extending slightly past the circle on all four sides; draw the circle and the cross lines with a thick line weight, about 1/12 of the icon width, the same weight as the document outline in the reference",
  pageDark: "a document page outline with a half-filled circle (contrast / dark mode symbol, left half solid, right half outline) in the middle",
  stamp: "a rubber hand stamp seen from the side (round knob handle on top, flat wide base) pressing down on a horizontal line below it",
  dropdown: "a wide rounded rectangle (a combo box / form dropdown field) with a small downward-pointing chevron at its right end and a short text line at its left",
  squiggle: "a capital letter A above a wavy squiggly underline, meaning squiggly text markup",
};

const [outArg, ...names] = process.argv.slice(2);
if (!outArg || !names.length) {
  console.error("usage: node generate.mjs <outDir> <name>...   names: " + Object.keys(concepts).join(" "));
  process.exit(1);
}
const out = path.resolve(outArg);
fs.mkdirSync(out, { recursive: true });

const run = (name) =>
  new Promise((done) => {
    if (!concepts[name]) return console.error(`unknown glyph ${name}`), done();
    const dest = path.join(out, `${name}.png`).split(path.sep).join("/");
    const prompt = `Use the attached image only as a style reference: it shows Microsoft Fluent UI System Icons, a Regular (outline) row on top and the matching Filled (solid) row below.
Generate ONE new image, wide landscape, pure white background, containing exactly two icons side by side with a wide gap: on the left the REGULAR (outline) version, on the right the FILLED (solid) version of the same icon.
Icon subject: ${concepts[name]}.
Style rules, strictly: Fluent UI System Icons look; flat pure black (#000000) shapes on pure white; no gradients, no shadows, no colour, no text labels, no background tiles; both icons the same size and vertically centred, each filling about 75% of its own square cell with equal padding; Regular uses a uniform line weight like the reference outline row with rounded line caps and softly rounded corners; Filled uses solid shapes with the small details knocked out in white, like the reference filled row; simple and readable at 20 pixels, as few details as possible.
After generating, copy the generated PNG file to exactly this path: ${dest}`;
    const p = spawn("codex", ["exec", "--skip-git-repo-check", "-i", ref, "-C", out, "-"], { shell: true, stdio: ["pipe", "pipe", "pipe"] });
    const log = fs.createWriteStream(path.join(out, `${name}.log`));
    p.stdout.pipe(log);
    p.stderr.pipe(log);
    p.stdin.end(prompt);
    p.on("close", (code) => {
      console.log(name, "exit", code, fs.existsSync(dest) ? "OK" : "NO FILE");
      done();
    });
  });

const queue = [...names];
await Promise.all(
  Array.from({ length: Math.min(4, queue.length) }, async () => {
    while (queue.length) await run(queue.shift());
  }),
);

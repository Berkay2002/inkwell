// Traces the 2-up PNGs from generate.mjs (Regular | Filled) into <name>.svg and <name>-filled.svg: one filled path
// on a 20 x 20 grid, the glyph fitted into 16 x 16 units and centred (2 units of air, as Fluent's 20 px icons have).
//   npm i potrace --prefix <tmp>             (not a dependency of the app; pure JS, with jimp)
//   NODE_PATH=<tmp>/node_modules node scripts/icons-gen/trace.cjs <rawDir> <outDir> <name>...
const fs = require("fs");
const path = require("path");
const potraceDir = path.dirname(require.resolve("potrace"));
const Jimp = require(require.resolve("jimp", { paths: [potraceDir] }));
const { Potrace } = require("potrace");

const K = 64; // trace at 64 px per grid unit
const GRID = 20;
const FIT = 16;
const [rawDir, outDir, ...names] = process.argv.slice(2);
if (!rawDir || !outDir || !names.length) {
  console.error("usage: trace.cjs <rawDir> <outDir> <name>...");
  process.exit(1);
}
fs.mkdirSync(outDir, { recursive: true });

const dark = (img, x, y) => img.bitmap.data[(y * img.bitmap.width + x) * 4] < 140;
function bbox(img, x0, x1) {
  let minX = 1e9, minY = 1e9, maxX = -1, maxY = -1;
  for (let y = 0; y < img.bitmap.height; y++)
    for (let x = x0; x < x1; x++)
      if (dark(img, x, y)) {
        minX = Math.min(minX, x); maxX = Math.max(maxX, x);
        minY = Math.min(minY, y); maxY = Math.max(maxY, y);
      }
  return { minX, minY, maxX, maxY };
}
/** Where the two icons part: the widest empty column run in the middle of the black span. */
function splitX(img) {
  const w = img.bitmap.width;
  const col = new Array(w).fill(false);
  for (let x = 0; x < w; x++) for (let y = 0; y < img.bitmap.height; y++) if (dark(img, x, y)) { col[x] = true; break; }
  const all = bbox(img, 0, w);
  const span = all.maxX - all.minX;
  let best = { len: 0, at: Math.round((all.minX + all.maxX) / 2) };
  for (let x = Math.round(all.minX + span * 0.3), run = 0, start = 0; x < all.minX + span * 0.7; x++) {
    if (!col[x]) {
      if (!run) start = x;
      run++;
      if (run > best.len) best = { len: run, at: start + Math.round(run / 2) };
    } else run = 0;
  }
  return best.at;
}

async function one(name) {
  const img = await Jimp.read(path.join(rawDir, `${name}.png`));
  const cut = splitX(img);
  for (const [variant, x0, x1] of [["regular", 0, cut], ["filled", cut, img.bitmap.width]]) {
    const b = bbox(img, x0, x1);
    const w = b.maxX - b.minX + 1;
    const h = b.maxY - b.minY + 1;
    const glyph = img.clone().crop(b.minX, b.minY, w, h);
    const s = (FIT * K) / Math.max(w, h);
    glyph.resize(Math.max(1, Math.round(w * s)), Math.max(1, Math.round(h * s)), Jimp.RESIZE_BICUBIC);
    const canvas = new Jimp(GRID * K, GRID * K, 0xffffffff);
    canvas.composite(glyph, Math.round((GRID * K - glyph.bitmap.width) / 2), Math.round((GRID * K - glyph.bitmap.height) / 2));
    const p = new Potrace({ turdSize: 120, optTolerance: 0.35, alphaMax: 1, threshold: 128, blackOnWhite: true });
    await new Promise((res, rej) => p.loadImage(canvas, (e) => (e ? rej(e) : res())));
    const tag = p.getPathTag("black", { x: 1 / K, y: 1 / K });
    const d = /d="([^"]*)"/.exec(tag)[1].replace(/-?\d+\.\d+/g, (n) => String(+(+n).toFixed(2)));
    const file = variant === "regular" ? `${name}.svg` : `${name}-filled.svg`;
    fs.writeFileSync(path.join(outDir, file), `<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20"><path d="${d}" fill-rule="evenodd"/></svg>\n`);
  }
  console.log(name, "traced");
}

(async () => {
  for (const n of names) await one(n);
})().catch((e) => {
  console.error(e);
  process.exit(1);
});

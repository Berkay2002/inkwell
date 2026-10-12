// Vendors the Fluent UI System Icons (MIT) that Inkwell uses into src/assets/icons/fluent/.
//   node scripts/vendor-fluent-icons.mjs                 downloads @fluentui/svg-icons@<version> with npm pack
//   node scripts/vendor-fluent-icons.mjs --from <dir>    reads an unpacked package's icons/ folder instead
// Which glyphs: the "fluent" map in src/assets/icons/manifest.json (our name -> Fluent snake_case name). For each one
// it copies <name>_<16|20|24>_<regular|filled>.svg. The 20 px pair is required; a grid Fluent does not draw for a
// glyph is skipped and the Icon component falls back to the nearest one. The files are copied as they are (LF, one final newline).
// The licence is third_party/fluent/LICENSE-MIT (the npm package ships none; it is Microsoft's repo LICENSE).
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifestPath = path.join(root, "src/assets/icons/manifest.json");
const dest = path.join(root, "src/assets/icons/fluent");
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

const fromAt = process.argv.indexOf("--from");
let iconsDir = fromAt > 0 ? path.resolve(process.argv[fromAt + 1]) : null;
if (!iconsDir) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fluent-icons-"));
  const spec = `@fluentui/svg-icons@${manifest.fluentVersion}`;
  console.log(`npm pack ${spec}`);
  const tgz = execFileSync("npm", ["pack", spec, "--silent", "--pack-destination", tmp], { shell: true, encoding: "utf8" }).trim().split(/\r?\n/).pop();
  execFileSync("tar", ["-xzf", path.join(tmp, tgz), "-C", tmp, "package/icons"], { stdio: "inherit" });
  iconsDir = path.join(tmp, "package", "icons");
}

const names = [...new Set(Object.values(manifest.fluent))].sort();
const want = [];
const problems = [];
for (const n of names) {
  for (const style of ["regular", "filled"]) {
    for (const grid of [16, 20, 24]) {
      const file = `${n}_${grid}_${style}.svg`;
      if (fs.existsSync(path.join(iconsDir, file))) want.push(file);
      else if (grid === 20) problems.push(`missing ${file}`);
    }
  }
}
for (const f of want) {
  const svg = fs.readFileSync(path.join(iconsDir, f), "utf8");
  const shapes = svg.match(/<(?!svg|\/svg)[a-zA-Z]+/g) ?? [];
  if (shapes.some((s) => s !== "<path")) problems.push(`${f}: has elements other than <path> (${[...new Set(shapes)].join(", ")})`);
}
if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}

fs.mkdirSync(dest, { recursive: true });
for (const old of fs.readdirSync(dest)) if (old.endsWith(".svg") && !want.includes(old)) fs.rmSync(path.join(dest, old));
for (const f of want) fs.writeFileSync(path.join(dest, f), fs.readFileSync(path.join(iconsDir, f), "utf8").replace(/\r\n/g, "\n").trimEnd() + "\n");
console.log(`${names.length} glyphs, ${want.length} files in ${path.relative(root, dest)}`);

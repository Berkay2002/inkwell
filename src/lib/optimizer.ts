import type { AuditCategory, CompressAudit, CompressOptions, ImageClassSettings, ImageCodec } from "./api";

/**
 * Pure helpers for the Advanced settings of the Compress sheet (Acrobat's Optimized PDF): labels,
 * validation that mirrors the engine's, merging a saved preset over today's defaults, and the rows of
 * the space-usage table. The state itself lives in `toolsStore`.
 */

export const CATEGORY_LABEL: Record<AuditCategory, string> = {
  images: "Images",
  fonts: "Fonts",
  contentStreams: "Page content",
  forms: "Form fields",
  comments: "Comments",
  links: "Links",
  bookmarks: "Bookmarks",
  thumbnails: "Page thumbnails",
  embeddedFiles: "Attachments",
  metadata: "Metadata",
  structure: "Document structure",
  other: "Other",
};

export const GARBAGE_LABEL = [
  "Keep every object",
  "Remove unused objects",
  "Also renumber objects",
  "Also merge identical objects",
  "Also merge identical streams",
];

export const CODEC_LABEL: Record<ImageCodec, string> = {
  retain: "Keep the encoding",
  jpeg: "JPEG",
  jpx: "JPEG 2000",
  flate: "Flate (lossless)",
  fax: "CCITT G4",
};

/** What each image class can be encoded as (the engine ignores the others). */
export const CODECS: Record<"color" | "gray" | "mono", ImageCodec[]> = {
  color: ["retain", "jpeg", "jpx", "flate"],
  gray: ["retain", "jpeg", "jpx", "flate"],
  mono: ["retain", "fax", "flate"],
};

export const FLATTEN_PPI = [72, 150, 300, 600];

const PPI_MIN = 36;
const PPI_MAX = 2400;

/** The same bounds the engine enforces: a sheet that passes this is not refused as `invalid-options`. */
export function validateOptions(o: CompressOptions): string | null {
  const names = { color: "Colour images", gray: "Grayscale images", mono: "Monochrome images" } as const;
  for (const k of ["color", "gray", "mono"] as const) {
    const c: ImageClassSettings = o.images[k];
    if (c.abovePpi !== 0) {
      if (!Number.isInteger(c.abovePpi) || !Number.isInteger(c.toPpi) || c.abovePpi < PPI_MIN || c.abovePpi > PPI_MAX || c.toPpi < PPI_MIN || c.toPpi > PPI_MAX) {
        return `${names[k]}: resolution must be between ${PPI_MIN} and ${PPI_MAX} ppi`;
      }
      if (c.toPpi > c.abovePpi) return `${names[k]}: the target resolution is above the threshold`;
    }
    if ((c.codec === "jpeg" || c.codec === "jpx") && (!Number.isInteger(c.quality) || c.quality < 1 || c.quality > 100)) {
      return `${names[k]}: quality must be 1 to 100`;
    }
  }
  if (o.transparency.flatten && (o.transparency.ppi < 36 || o.transparency.ppi > 600)) {
    return "Transparency: resolution must be between 36 and 600 ppi";
  }
  return null;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * `saved` laid over `base`: a key the saved preset lacks (it was saved before the engine had that
 * setting) keeps today's value, a key `base` does not know is dropped, and a value of the wrong type
 * falls back to `base`. The result has exactly the shape of `base`.
 */
export function normalizeOptions<T>(base: T, saved: unknown): T {
  if (Array.isArray(base)) {
    return (Array.isArray(saved) && saved.every((v) => typeof v === "string") ? saved : base) as T;
  }
  if (isObject(base)) {
    const s = isObject(saved) ? saved : {};
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(base)) out[k] = k in s ? normalizeOptions(base[k], s[k]) : base[k];
    return out as T;
  }
  return (typeof saved === typeof base && saved !== null ? saved : base) as T;
}

/** Deep equality for options (plain data only). */
export function sameOptions(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((v, i) => sameOptions(v, b[i]));
  if (isObject(a) && isObject(b)) {
    const ka = Object.keys(a);
    return ka.length === Object.keys(b).length && ka.every((k) => k in b && sameOptions(a[k], b[k]));
  }
  return false;
}

/** What a saved preset keeps: fonts picked by name belong to one document. */
export function forPreset(o: CompressOptions): CompressOptions {
  return { ...o, preset: "custom", unembedFonts: [] };
}

export interface UsageRow {
  category: AuditCategory;
  label: string;
  bytes: number;
  percent: number;
}

/** The audit's non-empty categories, largest first. Percent is recomputed from the bytes when missing. */
export function usageRows(a: CompressAudit): UsageRow[] {
  return a.categories
    .filter((c) => c.bytes > 0)
    .map((c) => ({
      category: c.category,
      label: CATEGORY_LABEL[c.category] ?? c.category,
      bytes: c.bytes,
      percent: typeof c.percent === "number" ? c.percent : a.total > 0 ? (c.bytes * 100) / a.total : 0,
    }))
    .sort((x, y) => y.bytes - x.bytes);
}

/** How many discard options are on. */
export const discardCount = (o: CompressOptions) => Object.values(o.strip).filter(Boolean).length;

/** One short line per section for the sheet's collapsed headings. */
export function summarize(o: CompressOptions) {
  const cls = (c: ImageClassSettings) =>
    `${c.abovePpi > 0 ? `${c.abovePpi}→${c.toPpi} ppi` : "no downsampling"}, ${CODEC_LABEL[c.codec].toLowerCase()}`;
  const unembed = o.unembedAll ? "unembed all" : o.unembedFonts.length > 0 ? `unembed ${o.unembedFonts.length}` : "";
  const fonts = [o.subsetFonts ? "subset" : "", unembed].filter(Boolean).join(", ");
  return {
    images: cls(o.images.color),
    fonts: fonts || "unchanged",
    transparency: o.transparency.flatten ? `flatten at ${o.transparency.ppi} ppi` : "unchanged",
    discard: discardCount(o) === 0 ? "nothing" : `${discardCount(o)} on`,
    cleanup: o.garbage === 0 && !o.objectStreams && !o.compressStreams ? "off" : GARBAGE_LABEL[o.garbage]?.toLowerCase() ?? "",
  };
}

/** The discard options in the order the sheet lists them. */
export const DISCARD: { key: keyof CompressOptions["strip"]; label: string; hint: string }[] = [
  { key: "metadata", label: "Document information and metadata", hint: "Title, author, XMP and application data" },
  { key: "thumbnails", label: "Embedded page thumbnails", hint: "Viewers draw their own" },
  { key: "embeddedFiles", label: "File attachments", hint: "Embedded files and attachment annotations" },
  { key: "javascript", label: "JavaScript actions", hint: "Document, page and field scripts" },
  { key: "bookmarks", label: "Bookmarks", hint: "The outline tree" },
  { key: "formActions", label: "Form submit, reset and import actions", hint: "Fields keep their values" },
  { key: "alternateImages", label: "Alternate images", hint: "Print and web variants kept next to an image" },
  { key: "privateData", label: "Private data of other applications", hint: "What authoring tools left behind" },
  { key: "links", label: "All links", hint: "Every link annotation" },
];

import { describe, expect, it } from "vitest";
import type { CompressAudit, CompressOptions } from "./api";
import { discardCount, forPreset, normalizeOptions, sameOptions, summarize, usageRows, validateOptions } from "./optimizer";

const cls = (abovePpi: number, toPpi: number, codec: CompressOptions["images"]["color"]["codec"], quality: number) => ({
  abovePpi,
  toPpi,
  codec,
  quality,
});

/** What the engine's Balanced preset serializes to. */
export const balanced = (): CompressOptions => ({
  preset: "balanced",
  images: { color: cls(225, 150, "jpx", 75), gray: cls(225, 150, "jpx", 75), mono: cls(450, 300, "fax", 0), onlyIfSmaller: true },
  subsetFonts: false,
  unembedFonts: [],
  unembedAll: false,
  transparency: { flatten: false, ppi: 150 },
  compressStreams: true,
  objectStreams: true,
  garbage: 4,
  strip: {
    metadata: false,
    thumbnails: false,
    embeddedFiles: false,
    javascript: false,
    bookmarks: false,
    formActions: false,
    alternateImages: false,
    privateData: false,
    links: false,
    invalidLinks: false,
  },
  visualPages: 8,
});

describe("validateOptions", () => {
  it("accepts the engine's own presets and refuses what the engine would refuse", () => {
    expect(validateOptions(balanced())).toBeNull();
    const o = balanced();
    o.images.color.abovePpi = 20;
    expect(validateOptions(o)).toMatch(/Colour images: resolution/);
    const p = balanced();
    p.images.gray.toPpi = 300;
    expect(validateOptions(p)).toMatch(/Grayscale images: the target resolution is above/);
    const q = balanced();
    q.images.color.quality = 0;
    expect(validateOptions(q)).toMatch(/quality/);
    const t = balanced();
    t.transparency = { flatten: true, ppi: 10 };
    expect(validateOptions(t)).toMatch(/Transparency/);
  });

  it("does not look at a class that is not downsampled or not lossy", () => {
    const o = balanced();
    o.images.mono = cls(0, 0, "flate", 0);
    o.images.color = cls(0, 0, "retain", 0);
    o.transparency = { flatten: false, ppi: 1 };
    expect(validateOptions(o)).toBeNull();
  });
});

describe("normalizeOptions", () => {
  it("lays a preset saved by an older build over today's defaults", () => {
    const old = {
      preset: "custom",
      images: { color: { abovePpi: 150, toPpi: 100, codec: "jpeg", quality: 50 }, onlyIfSmaller: false },
      strip: { metadata: true },
      garbage: 3,
      somethingFromTheFuture: { a: 1 },
    };
    const o = normalizeOptions(balanced(), old);
    expect(o.images.color).toEqual(cls(150, 100, "jpeg", 50));
    expect(o.images.gray).toEqual(balanced().images.gray);
    expect(o.images.onlyIfSmaller).toBe(false);
    expect(o.strip.metadata).toBe(true);
    expect(o.strip.links).toBe(false);
    expect(o.garbage).toBe(3);
    expect(o.compressStreams).toBe(true);
    expect(Object.keys(o).sort()).toEqual(Object.keys(balanced()).sort());
  });

  it("falls back to the default for a value of the wrong type, and for junk", () => {
    const o = normalizeOptions(balanced(), { garbage: "lots", unembedFonts: [1, 2], images: "no", subsetFonts: null });
    expect(o).toEqual(balanced());
    expect(normalizeOptions(balanced(), null)).toEqual(balanced());
    expect(normalizeOptions(balanced(), 12)).toEqual(balanced());
  });

  it("round-trips: what is saved comes back the same", () => {
    const o = balanced();
    o.images.color = cls(300, 200, "jpeg", 40);
    o.unembedAll = true;
    o.transparency = { flatten: true, ppi: 300 };
    o.strip.links = true;
    o.strip.invalidLinks = true;
    const json = JSON.parse(JSON.stringify(forPreset(o)));
    expect(normalizeOptions(balanced(), json)).toEqual(forPreset(o));
  });
});

describe("helpers", () => {
  it("forPreset drops the document's font names and marks the settings custom", () => {
    const o = { ...balanced(), unembedFonts: ["ABCDEF+Arial"], unembedAll: true };
    const p = forPreset(o);
    expect(p.preset).toBe("custom");
    expect(p.unembedFonts).toEqual([]);
    expect(p.unembedAll).toBe(true);
  });

  it("sameOptions compares deeply", () => {
    expect(sameOptions(balanced(), balanced())).toBe(true);
    const o = balanced();
    o.images.mono.toPpi = 200;
    expect(sameOptions(balanced(), o)).toBe(false);
    const f = { ...balanced(), unembedFonts: ["A"] };
    expect(sameOptions(balanced(), f)).toBe(false);
  });

  it("counts the discard options that are on and summarizes the sections", () => {
    const o = balanced();
    expect(discardCount(o)).toBe(0);
    expect(summarize(o).discard).toBe("nothing");
    o.strip.links = true;
    o.strip.metadata = true;
    o.subsetFonts = true;
    o.unembedAll = true;
    o.transparency.flatten = true;
    const s = summarize(o);
    expect(discardCount(o)).toBe(2);
    expect(s.discard).toBe("2 on");
    expect(s.fonts).toBe("subset, unembed all");
    expect(s.transparency).toBe("flatten at 150 ppi");
    expect(s.images).toBe("225→150 ppi, jpeg 2000");
  });

  it("lists the non-empty categories largest first with percentages that add up", () => {
    const a: CompressAudit = {
      total: 1000,
      images: 1,
      fonts: 1,
      largest: [],
      fontList: [],
      categories: [
        { category: "fonts", bytes: 100, objects: 2, percent: 10 },
        { category: "images", bytes: 800, objects: 1, percent: 80 },
        { category: "links", bytes: 0, objects: 0, percent: 0 },
        { category: "structure", bytes: 100, objects: 40, percent: 10 },
      ],
    } as never;
    const rows = usageRows(a);
    expect(rows.map((r) => r.category)).toEqual(["images", "fonts", "structure"]);
    expect(rows[0].label).toBe("Images");
    expect(rows.reduce((n, r) => n + r.percent, 0)).toBe(100);
    expect(rows.reduce((n, r) => n + r.bytes, 0)).toBe(a.total);
  });
});

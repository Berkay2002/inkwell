import { describe, expect, it } from "vitest";
import type { OcrCatalog, OcrDetection, OcrLang, OcrPlan, OcrUnsure } from "./api";
import {
  canStart,
  changesPicture,
  keptLine,
  previewPage,
  reviewOrder,
  unsureReason,
  detectedLine,
  engineLabel,
  engineLine,
  initialLanguages,
  languageName,
  matchLanguages,
  missingMatch,
  ocrProgress,
  primary,
  requestOptions,
  toggleLanguage,
} from "./ocrLogic";

const lang = (tag: string, windows: string | null, tesseract = false, extra: Partial<OcrLang> = {}): OcrLang => ({
  tag,
  windows,
  tesseract,
  rtl: false,
  cjk: false,
  experimental: false,
  ...extra,
});
const catalog: OcrCatalog = {
  languages: [lang("en", "en-US", true), lang("sv", "sv-SE"), lang("ar", null, true, { rtl: true, experimental: true }), lang("de", null, true)],
  tesseractFound: true,
  tessdataDir: "C:\\Tesseract\\tessdata",
};
const detection = (o: Partial<OcrDetection>): OcrDetection => ({ languages: ["sv", "en"], source: "profile", guess: null, confidence: 0, ...o });

describe("language names and engines", () => {
  it("shows a name for a tag and falls back to the tag", () => {
    expect(languageName("sv")).toBe("Swedish");
    expect(languageName("sv-SE")).toBe("Swedish");
    expect(languageName("zzzz")).toBe("zzzz");
    expect(primary("en-US")).toBe("en");
    expect(primary("zh_Hans")).toBe("zh");
  });

  it("keeps a Tesseract data name with a script whole, and names it", () => {
    expect(primary("chi_tra")).toBe("chi_tra");
    expect(primary("srp_latn")).toBe("srp_latn");
    expect(primary("JPN_vert")).toBe("jpn_vert");
    expect(languageName("chi_tra")).toBe("Chinese (Traditional)");
    expect(languageName("jpn_vert")).toBe("Japanese (vertical)");
    expect(languageName("srp_latn")).toBe("Serbian (Latin)");
  });

  it("says which engine serves a language", () => {
    expect(engineLabel({ windows: "sv-SE", tesseract: false })).toBe("Windows");
    expect(engineLabel({ windows: null, tesseract: true })).toBe("Tesseract");
    expect(engineLabel({ windows: "en-US", tesseract: true })).toBe("Windows, Tesseract");
  });
});

describe("searching the list", () => {
  it("matches names and tags, every word of the query, case-insensitively", () => {
    expect(matchLanguages(catalog.languages, "swe").map((l) => l.tag)).toEqual(["sv"]);
    expect(matchLanguages(catalog.languages, "GER").map((l) => l.tag)).toEqual(["de"]);
    expect(matchLanguages(catalog.languages, "ar").map((l) => l.tag)).toEqual(["ar"]);
    expect(matchLanguages(catalog.languages, "  ").length).toBe(catalog.languages.length);
    expect(matchLanguages(catalog.languages, "swedish english")).toEqual([]);
  });

  it("recognizes a language that is known but not installed", () => {
    expect(missingMatch(catalog, "japan")).toBe("ja");
    expect(missingMatch(catalog, "ja")).toBe("ja");
    // installed ones and one-letter queries are not "missing"
    expect(missingMatch(catalog, "swed")).toBeNull();
    expect(missingMatch(catalog, "j")).toBeNull();
    expect(missingMatch(catalog, "qqq")).toBeNull();
  });
});

describe("the starting selection", () => {
  const defaults = ["sv", "en"];

  it("takes what the document is written in, when it was read from the document", () => {
    expect(initialLanguages({ detection: detection({ source: "text", guess: "de" }), saved: ["sv"], defaults, catalog })).toEqual(["de"]);
    expect(initialLanguages({ detection: detection({ source: "sample", guess: "en" }), saved: null, defaults, catalog })).toEqual(["en"]);
  });

  it("falls back to the last choice, if it is still installed", () => {
    expect(initialLanguages({ detection: detection({}), saved: ["de", "sv"], defaults, catalog })).toEqual(["de", "sv"]);
    expect(initialLanguages({ detection: null, saved: ["ja", "de"], defaults, catalog })).toEqual(["de"]);
  });

  it("then to the display language plus English, then to the detection's own list", () => {
    expect(initialLanguages({ detection: detection({}), saved: null, defaults, catalog })).toEqual(["sv", "en"]);
    expect(initialLanguages({ detection: detection({ languages: ["en"] }), saved: null, defaults: [], catalog })).toEqual(["en"]);
    expect(initialLanguages({ detection: null, saved: null, defaults: [], catalog })).toEqual([]);
  });

  it("migrates old tags (en-US) to primary tags", () => {
    expect(initialLanguages({ detection: null, saved: ["sv-SE"], defaults, catalog })).toEqual(["sv"]);
  });
});

describe("toggling a language", () => {
  it("appends in the order chosen and removes in place", () => {
    expect(toggleLanguage(["sv"], "en")).toEqual(["sv", "en"]);
    expect(toggleLanguage(["sv", "en", "de"], "en")).toEqual(["sv", "de"]);
    expect(toggleLanguage([], "de")).toEqual(["de"]);
  });
});

describe("what the sheet says", () => {
  it("tells where the detected language came from", () => {
    expect(detectedLine(null, true)).toMatch(/Looking/);
    expect(detectedLine(detection({ source: "text", guess: "sv" }), false)).toBe("Looks like Swedish, from the text already in this file.");
    expect(detectedLine(detection({ source: "sample", guess: "de" }), false)).toBe("Looks like German, from a sample of the first page.");
    expect(detectedLine(detection({}), false)).toBe("Chosen from your Windows languages.");
    expect(detectedLine(null, false)).toBe("");
  });

  it("names the engine, or says there is none", () => {
    const plan = (engine: OcrPlan["engine"]): OcrPlan => ({ engine, languages: ["sv"], dropped: [], missing: [], note: null });
    expect(engineLine(plan("windows"))).toBe("Read with Windows OCR.");
    expect(engineLine(plan("tesseract"))).toBe("Read with Tesseract.");
    expect(engineLine(plan(null))).toMatch(/No installed engine/);
    expect(engineLine(null)).toBe("");
  });
});

describe("the request", () => {
  it("passes style and dpi, and redoes pages only when asked and allowed", () => {
    expect(requestOptions({ style: "searchable", dpi: 200, skipText: true })).toEqual({ style: "searchable", dpi: 200, force: false });
    expect(requestOptions({ style: "cleaned", dpi: 300, skipText: false })).toEqual({ style: "cleaned", dpi: 300, force: true });
  });

  it("never redoes pages that have text when making editable text", () => {
    expect(requestOptions({ style: "editable", dpi: 150, skipText: false }).force).toBe(false);
  });

  it("starts only with a language, an engine for it and a valid range", () => {
    const ok: OcrPlan = { engine: "windows", languages: ["en"], dropped: [], missing: [], note: null };
    expect(canStart({ languages: ["en"], plan: ok, rangeOk: true })).toBe(true);
    expect(canStart({ languages: [], plan: ok, rangeOk: true })).toBe(false);
    expect(canStart({ languages: ["en"], plan: null, rangeOk: true })).toBe(false);
    expect(canStart({ languages: ["en"], plan: { ...ok, engine: null }, rangeOk: true })).toBe(false);
    expect(canStart({ languages: ["en"], plan: ok, rangeOk: false })).toBe(false);
  });
});

describe("the preview and the review list", () => {
  it("previews the page in view when it is chosen, else the first chosen", () => {
    expect(previewPage(null, 4)).toBe(4);
    expect(previewPage([2, 3, 4], 3)).toBe(3);
    expect(previewPage([5, 6], 1)).toBe(5);
    expect(previewPage([], 2)).toBe(2);
  });

  it("keeps the last fifth of the bar for writing the pages of a picture style", () => {
    expect(ocrProgress({ done: 5, total: 10 }, "searchable")).toBe(0.5);
    expect(ocrProgress({ done: 10, total: 10 }, "cleaned")).toBeCloseTo(0.8);
    expect(ocrProgress({ done: 5, total: 10, writing: true }, "editable")).toBeCloseTo(0.9);
    expect(ocrProgress({ done: 10, total: 10, writing: true }, "cleaned")).toBeCloseTo(1);
    expect(ocrProgress({ done: 0, total: 0 }, "cleaned")).toBe(0);
  });

  it("offers a preview only for the styles that change the picture", () => {
    expect(changesPicture("searchable")).toBe(false);
    expect(changesPicture("cleaned")).toBe(true);
    expect(changesPicture("editable")).toBe(true);
  });

  it("says how many words stayed in the picture", () => {
    expect(keptLine({ words_kept: 0 })).toBe("");
    expect(keptLine({ words_kept: 1 })).toBe("1 word kept as image (unsure)");
    expect(keptLine({ words_kept: 12 })).toBe("12 words kept as image (unsure)");
    expect(unsureReason("picture")).toMatch(/picture/);
  });

  it("walks the unsure words from the page in view, top to bottom", () => {
    const w = (page: number, y: number, x: number): OcrUnsure => ({ page, text: `${page}-${y}-${x}`, bbox: [x, y, x + 10, y + 10], why: "low-confidence" });
    const order = reviewOrder([w(0, 50, 0), w(2, 10, 0), w(1, 40, 5), w(1, 40, 2), w(1, 20, 0)], 1).map((u) => u.text);
    expect(order).toEqual(["1-20-0", "1-40-2", "1-40-5", "0-50-0", "2-10-0"]);
  });
});

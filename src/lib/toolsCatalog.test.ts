import { describe, expect, it } from "vitest";
import {
  MAX_PINNED,
  MAX_RECENT,
  RESULTS,
  type ToolItem,
  cleanIds,
  filterTools,
  groupTools,
  pick,
  pushRecent,
  searchTools,
  togglePin,
  tokens,
} from "./toolsCatalog";

const mk = (id: string, label: string, group: string, description: string, keywords?: string): ToolItem => ({
  id,
  label,
  group,
  icon: "tools",
  description,
  keywords,
  onSelect: () => {},
});

// the same entries (and synonyms) as lib/toolItems.ts, without the stores
const items: ToolItem[] = [
  mk("compress", "Compress file", "optimize", "Make the file smaller to email or upload", "shrink smaller reduce size optimize email upload"),
  mk("ocr", "Recognize text", "optimize", "Make scanned pages searchable and copyable", "ocr scan scanned searchable read text image"),
  mk("header-footer", "Header & footer", "marks", "Add page numbers, dates or text to every page", "page numbers date running title"),
  mk("watermark", "Watermark", "marks", "Stamp text like DRAFT or CONFIDENTIAL across pages", "stamp draft confidential text image overlay"),
  mk("background", "Background", "marks", "Put a colour or image behind page content", "colour color image paper behind"),
  mk("bates", "Bates numbering", "marks", "Number pages for legal filings", "legal numbers page numbers court prefix"),
  mk("protect", "Protect with a password", "protect", "Require a password to open the file", "encrypt lock secure password open"),
  mk("export", "Export", "convert", "Save as Word, images, text and more", "save as word docx image jpg png text html convert"),
  mk("compare", "Compare files", "check", "Highlight the differences between two PDFs", "diff differences changes versions side by side"),
];
const ids = (r: ToolItem[]) => r.map((i) => i.id);

describe("tokens", () => {
  it("lower-cases, splits on punctuation and takes a plain plural or -ing off", () => {
    expect(tokens("Header & footer")).toEqual(["header", "footer"]);
    expect(tokens("Page Numbers")).toEqual(["page", "number"]);
    expect(tokens("numbering")).toEqual(["number"]);
    expect(tokens("  ")).toEqual([]);
  });
});

describe("searchTools", () => {
  it("finds Compress for the words people use for it", () => {
    for (const q of ["smaller", "shrink", "reduce size", "email"]) expect(ids(searchTools(items, q))[0]).toBe("compress");
  });
  it("finds Recognize text for OCR and scan", () => {
    expect(ids(searchTools(items, "OCR"))[0]).toBe("ocr");
    expect(ids(searchTools(items, "scan"))[0]).toBe("ocr");
  });
  it("finds Watermark for stamp and draft", () => {
    expect(ids(searchTools(items, "stamp"))[0]).toBe("watermark");
    expect(ids(searchTools(items, "draft"))[0]).toBe("watermark");
  });
  it("finds header & footer and Bates for page numbers", () => {
    const r = ids(searchTools(items, "page numbers"));
    expect(r).toContain("header-footer");
    expect(r).toContain("bates");
    // the name beats a synonym: "number" starts Bates numbering's name
    expect(searchTools(items, "numbering")[0].id).toBe("bates");
  });
  it("needs every word, and a name match outranks a description match", () => {
    expect(searchTools(items, "password zzz")).toEqual([]);
    // "text" is in Recognize text's name but only a keyword or description elsewhere
    expect(searchTools(items, "text")[0].id).toBe("ocr");
  });
  it("matches a prefix and the group title", () => {
    expect(ids(searchTools(items, "wat"))).toEqual(["watermark"]);
    expect(ids(searchTools(items, "protect"))[0]).toBe("protect");
    expect(ids(searchTools(items, "marks"))).toEqual(["header-footer", "watermark", "background", "bates"]);
  });
  it("returns everything in catalog order for an empty query", () => {
    expect(searchTools(items, "  ")).toEqual(items);
  });
});

describe("groupTools / filterTools", () => {
  it("lists groups in their fixed order and skips empty ones", () => {
    const sec = groupTools([items[6], items[0], items[2]]);
    expect(sec.map((s) => s.id)).toEqual(["optimize", "marks", "protect"]);
    expect(sec[1].title).toBe("Page marks");
  });
  it("gives a group it does not know a header of its own at the end", () => {
    const sec = groupTools([mk("f", "Fill form", "forms", "Fill in fields"), items[0]]);
    expect(sec.map((s) => s.title)).toEqual(["Optimize", "forms"]);
  });
  it("shows the groups for an empty box and one ranked Results section for a query", () => {
    expect(filterTools(items, "").map((s) => s.id)).toEqual(["optimize", "marks", "protect", "convert", "check"]);
    const r = filterTools(items, "smaller");
    expect(r).toHaveLength(1);
    expect(r[0].id).toBe(RESULTS);
    expect(ids(r[0].items)).toEqual(["compress"]);
    expect(filterTools(items, "qqqq")).toEqual([]);
  });
});

describe("recent and pinned lists", () => {
  it("puts the newest first, drops the repeat and keeps four", () => {
    let l: string[] = [];
    for (const id of ["a", "b", "c", "d"]) l = pushRecent(l, id);
    expect(l).toEqual(["d", "c", "b", "a"]);
    expect(pushRecent(l, "b")).toEqual(["b", "d", "c", "a"]);
    expect(pushRecent(l, "e")).toEqual(["e", "d", "c", "b"]);
    expect(pushRecent(l, "e")).toHaveLength(MAX_RECENT);
  });
  it("pins up to three and refuses a fourth without dropping a pin", () => {
    let l: string[] = [];
    for (const id of ["a", "b", "c"]) l = togglePin(l, id).pinned;
    expect(l).toEqual(["a", "b", "c"]);
    expect(l).toHaveLength(MAX_PINNED);
    const over = togglePin(l, "d");
    expect(over).toEqual({ pinned: ["a", "b", "c"], full: true });
    expect(togglePin(l, "b")).toEqual({ pinned: ["a", "c"], full: false });
    expect(togglePin(["a", "c"], "d")).toEqual({ pinned: ["a", "c", "d"], full: false });
  });
  it("cleans a list read back from storage", () => {
    expect(cleanIds(["a", 3, "a", "b", null, "c", "d"], 3)).toEqual(["a", "b", "c"]);
    expect(cleanIds("nope", 3)).toEqual([]);
    expect(cleanIds(undefined, 3)).toEqual([]);
  });
  it("picks items in id order and skips ids that are not tools", () => {
    expect(ids(pick(items, ["bates", "gone", "compress"]))).toEqual(["bates", "compress"]);
  });
});

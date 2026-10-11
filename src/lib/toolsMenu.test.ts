import { describe, expect, it } from "vitest";
import { type ToolItem, filterTools } from "./toolsMenu";

const item = (id: string, label: string, group: string, keywords?: string): ToolItem => ({ id, label, group, keywords, onSelect: () => {} });

const ITEMS: ToolItem[] = [
  item("protect", "Protect with a password…", "protect"),
  item("compress", "Compress file…", "optimize"),
  item("ocr", "Recognize text…", "optimize", "ocr scan"),
  item("header", "Header & footer…", "marks"),
  item("bates", "Bates numbering…", "marks"),
  item("export", "Export…", "convert"),
];

const ids = (q: string) => filterTools(ITEMS, q).map((s) => [s.id, s.items.map((i) => i.id)]);

describe("tools menu filter", () => {
  it("an empty query keeps everything, under the headers in their order", () => {
    expect(ids("")).toEqual([
      ["optimize", ["compress", "ocr"]],
      ["marks", ["header", "bates"]],
      ["protect", ["protect"]],
      ["convert", ["export"]],
    ]);
    expect(ids("   ")).toEqual(ids(""));
  });

  it("matches the label, ignoring case and the ellipsis", () => {
    expect(ids("COMPRESS")).toEqual([["optimize", ["compress"]]]);
    expect(ids("export…")).toEqual([["convert", ["export"]]]);
  });

  it("every word has to match, in any order", () => {
    expect(ids("numbering bates")).toEqual([["marks", ["bates"]]]);
    expect(ids("header zebra")).toEqual([]);
  });

  it("matches keywords and the group's title", () => {
    expect(ids("ocr")).toEqual([["optimize", ["ocr"]]]);
    expect(ids("page marks")).toEqual([["marks", ["header", "bates"]]]);
    expect(ids("optimize")).toEqual([["optimize", ["compress", "ocr"]]]);
  });

  it("a group with nothing left has no header", () => {
    expect(filterTools(ITEMS, "password").map((s) => s.title)).toEqual(["Protect"]);
  });

  it("a group that is not listed gets a header of its own, after the listed ones", () => {
    const more = [...ITEMS, item("pdfa", "Preflight…", "pdfa"), item("pdfa2", "Convert to PDF/A…", "pdfa")];
    const sections = filterTools(more, "");
    expect(sections.map((s) => s.id)).toEqual(["optimize", "marks", "protect", "convert", "pdfa"]);
    expect(sections[4].items.map((i) => i.id)).toEqual(["pdfa", "pdfa2"]);
    expect(filterTools(more, "preflight").map((s) => s.id)).toEqual(["pdfa"]);
  });

  it("keeps the order the items were given in inside a group", () => {
    const rev = [...ITEMS].reverse();
    expect(filterTools(rev, "").find((s) => s.id === "optimize")!.items.map((i) => i.id)).toEqual(["ocr", "compress"]);
  });
});

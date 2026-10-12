import { describe, expect, it } from "vitest";
import { rankPalette } from "./paletteRank";

const items = [
  { title: "Edit links", words: "Links Add or change links on pages" },
  { title: "Go to page…" },
  { title: "Organize pages", words: "Organize pages Reorder, rotate, delete or insert pages" },
  { title: "Open recent.pdf", words: undefined },
];

describe("rankPalette", () => {
  it("a title match beats a description match: 'page' puts Organize pages before Edit links", () => {
    const got = rankPalette(items, "page").map((i) => i.title);
    expect(got.slice(0, 2)).toEqual(["Go to page…", "Organize pages"]);
    expect(got.indexOf("Organize pages")).toBeLessThan(got.indexOf("Edit links"));
  });

  it("a title that starts with the word comes first", () => {
    expect(rankPalette(items, "organ").map((i) => i.title)[0]).toBe("Organize pages");
    expect(rankPalette(items, "open")[0].title).toBe("Open recent.pdf");
  });

  it("every word must match, in the title or behind it", () => {
    expect(rankPalette(items, "edit pages").map((i) => i.title)).toEqual(["Edit links"]);
    expect(rankPalette(items, "zzz")).toEqual([]);
  });

  it("an empty query keeps the list as it is; ties keep its order", () => {
    expect(rankPalette(items, "  ")).toBe(items);
    const tie = [{ title: "Beta page" }, { title: "Alpha page" }];
    expect(rankPalette(tie, "page")).toEqual(tie);
  });
});

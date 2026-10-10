import { describe, expect, it } from "vitest";
import { MAX_UNPINNED, clean, formatSize, fromLegacy, isJunk, keyOf, ordered, parse, relTime, shortDir, upsert } from "./recent";

const R = (path: string, opened = 0, extra = {}) => ({ path, opened, ...extra });

describe("keyOf", () => {
  it("folds slash style, case, verbatim prefix and trailing separators", () => {
    const k = keyOf("C:\\Users\\Me\\Doc.pdf");
    expect(keyOf("c:/users/me/doc.PDF")).toBe(k);
    expect(keyOf("\\\\?\\C:\\Users\\Me\\Doc.pdf")).toBe(k);
    expect(keyOf("C:\\Users\\\\Me\\Doc.pdf")).toBe(k);
  });
  it("keeps the UNC double backslash", () => {
    expect(keyOf("\\\\?\\UNC\\Srv\\Share\\a.pdf")).toBe("\\\\srv\\share\\a.pdf");
    expect(keyOf("//srv/share/a.pdf")).toBe("\\\\srv\\share\\a.pdf");
  });
});

describe("list", () => {
  it("dedupes by key, keeps the pin and puts the newest first", () => {
    let l = upsert([], R("C:\\a.pdf", 1, { pinned: true }));
    l = upsert(l, R("C:\\b.pdf", 2));
    l = upsert(l, R("c:/A.pdf", 3));
    expect(l.map((r) => r.path)).toEqual(["c:/A.pdf", "C:\\b.pdf"]);
    expect(l[0].pinned).toBe(true);
  });
  it("caps unpinned entries but never pinned ones", () => {
    let l = upsert([], R("C:\\pinned.pdf", 0, { pinned: true }));
    for (let i = 1; i <= MAX_UNPINNED + 5; i++) l = upsert(l, R(`C:\\f${i}.pdf`, i));
    expect(l.length).toBe(MAX_UNPINNED + 1);
    expect(l.some((r) => r.path.endsWith("pinned.pdf"))).toBe(true);
    expect(ordered(l)[0].path).toBe("C:\\pinned.pdf");
  });
  it("cleans fixtures and duplicates", () => {
    const l = clean([R("C:\\Temp\\inkwell-bench\\big.pdf"), R("C:/x/inkwell-tests/a.pdf"), R("C:\\k.pdf"), R("c:/K.PDF")]);
    expect(l.map((r) => r.path)).toEqual(["C:\\k.pdf"]);
    expect(isJunk("C:\\Docs\\inkwell-notes.pdf")).toBe(false);
  });
  it("survives malformed storage", () => {
    expect(parse(null)).toBeNull();
    expect(parse("{")).toEqual([]);
    expect(parse(JSON.stringify([{ path: "a", opened: 1 }, { path: 3 }, null]))).toEqual([{ path: "a", opened: 1 }]);
    // imported entries keep their order but claim no open time (the card then shows no "just now")
    expect(fromLegacy(["a", 1, "b"])).toEqual([{ path: "a", opened: 0 }, { path: "b", opened: 0 }]);
  });
});

describe("display", () => {
  const now = new Date(2026, 9, 10, 15, 0).getTime();
  it("relative time", () => {
    expect(relTime(now - 20_000, now)).toBe("just now");
    expect(relTime(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(relTime(now - 2 * 3600_000, now)).toBe("2 h ago");
    expect(relTime(new Date(2026, 9, 9, 9, 0).getTime(), now)).toBe("Yesterday");
    expect(relTime(new Date(2026, 9, 6, 9, 0).getTime(), now)).toBe("4 days ago");
    expect(relTime(new Date(2026, 2, 1).getTime(), now)).toBe("1 Mar");
    expect(relTime(new Date(2025, 2, 1).getTime(), now)).toBe("1 Mar 2025");
  });
  it("sizes and folders", () => {
    expect(formatSize(999)).toBe("999 B");
    expect(formatSize(2_400_000)).toBe("2.4 MB");
    expect(formatSize(48_000_000)).toBe("48 MB");
    expect(shortDir("C:\\Users\\Me\\Documents\\Taxes")).toBe("…\\Documents\\Taxes");
    expect(shortDir("C:\\Docs")).toBe("C:\\Docs");
  });
});

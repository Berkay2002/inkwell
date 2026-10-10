import { describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

let next = 0;
const closeDocument = vi.fn((_d: number) => Promise.resolve());
// Like Rust: every spelling resolves to the one canonical path.
vi.mock("./api", () => ({
  openDocument: vi.fn(() => Promise.resolve({ status: "opened", doc: ++next, path: String.raw`C:\Docs\A.pdf`, title: null, pageCount: 1, firstPage: [10, 10] })),
  closeDocument,
  pageSizes: vi.fn(() => Promise.resolve()),
  cancelSearch: vi.fn(() => Promise.resolve()),
}));
vi.mock("../recents", () => ({ useRecents: { getState: () => ({ record: () => Promise.resolve() }) } }));

describe("openPath", () => {
  it("opens one tab for case, slash and .. spellings", async () => {
    const { useStore } = await import("../store");
    const st = useStore.getState();
    await st.openPath(String.raw`C:\Docs\A.pdf`);
    await st.openPath("c:/docs/a.PDF");
    await st.openPath(String.raw`C:\Docs\Sub\..\A.pdf`);
    expect(useStore.getState().error).toBeNull();
    expect(useStore.getState().order).toHaveLength(1);
    expect(closeDocument.mock.calls).toEqual([[2]]);
  });
});

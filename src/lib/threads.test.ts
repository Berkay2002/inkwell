import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ run: vi.fn((_id: number, f: () => Promise<unknown>) => f()) }));
vi.mock("../store", () => ({ useStore: { getState: () => ({ setUi: vi.fn() }) } }));

import { invoke } from "@tauri-apps/api/core";
import type { ArticleThread } from "./api";
import { beadNumber, beadPointer, beadZoom, nextTitle, otherToolHeld, propsChange, propsDraft, readsKeys, reconcile, sameInfo, stepBead, useThreads } from "./threads";

const inv = vi.mocked(invoke);
const bead = (id: number, page: number, w = 200) => ({ id, page, rect: [50, 100, 50 + w, 400] as [number, number, number, number] });
const thread = (id: number, title: string, beads: ReturnType<typeof bead>[]): ArticleThread => ({
  id,
  title,
  info: { title, subject: "", author: "", keywords: "" },
  beads,
  damaged: null,
});
// article 1 runs over three pages (two beads on the first), article 2 is one bead
const LIST = [thread(10, "Article 1", [bead(11, 0), bead(12, 0), bead(13, 1), bead(14, 3)]), thread(20, "Article 2", [bead(21, 2)])];

beforeEach(() => {
  vi.clearAllMocks();
  useThreads.setState({ docs: { 1: LIST }, show: false, tool: false, selected: null, follow: null, props: null });
});

describe("follow navigation", () => {
  it("steps to the next and the previous bead, across pages", () => {
    expect(stepBead(LIST, 10, 12, 1)?.id).toBe(13);
    expect(stepBead(LIST, 10, 13, 1)?.page).toBe(3);
    expect(stepBead(LIST, 10, 13, -1)?.id).toBe(12);
  });

  it("stays inside its thread and stops at both ends", () => {
    expect(stepBead(LIST, 10, 14, 1)).toBeNull();
    expect(stepBead(LIST, 10, 11, -1)).toBeNull();
    expect(stepBead(LIST, 20, 21, 1)).toBeNull();
    expect(stepBead(LIST, 10, 99, 1)).toBeNull();
  });

  it("the store moves on, ends reading at the last bead and stays at the first", () => {
    const t = useThreads.getState();
    t.startFollow(1, 10);
    expect(useThreads.getState().follow).toEqual({ doc: 1, thread: 10, bead: 11 });
    expect(t.step(-1)).toBe("start");
    expect(useThreads.getState().follow?.bead).toBe(11);
    for (const id of [12, 13, 14]) {
      expect(t.step(1)).toBe("moved");
      expect(useThreads.getState().follow?.bead).toBe(id);
    }
    expect(t.step(1)).toBe("end");
    expect(useThreads.getState().follow).toBeNull();
    expect(t.step(1)).toBe("none");
  });

  it("Esc leaves; starting from a given bead; an empty or unknown thread does not start", () => {
    const t = useThreads.getState();
    t.startFollow(1, 10, 13);
    expect(useThreads.getState().follow?.bead).toBe(13);
    t.stopFollow();
    expect(useThreads.getState().follow).toBeNull();
    t.startFollow(1, 999);
    expect(useThreads.getState().follow).toBeNull();
    useThreads.setState({ docs: { 1: [thread(30, "Empty", [])] } });
    t.startFollow(1, 30);
    expect(useThreads.getState().follow).toBeNull();
  });

  it("reading and the Article tool exclude each other", () => {
    const t = useThreads.getState();
    t.setTool(true);
    t.startFollow(1, 10);
    expect(useThreads.getState().tool).toBe(false);
    t.setTool(true);
    expect(useThreads.getState().follow).toBeNull();
  });
});

describe("reading the list again", () => {
  const afterDelete = LIST.map((t) => (t.id === 10 ? { ...t, beads: t.beads.filter((b) => b.id !== 12) } : t));

  it("reading moves to the neighbour of a deleted bead; the selection is dropped", async () => {
    useThreads.setState({ follow: { doc: 1, thread: 10, bead: 12 }, selected: { doc: 1, thread: 10, bead: 12 } });
    inv.mockResolvedValue(afterDelete);
    await useThreads.getState().load(1);
    expect(useThreads.getState().follow).toEqual({ doc: 1, thread: 10, bead: 13 });
    expect(useThreads.getState().selected).toBeNull();
  });

  it("everything about a deleted article goes with it", async () => {
    useThreads.setState({ follow: { doc: 1, thread: 20, bead: 21 }, props: { doc: 1, thread: 20 } });
    inv.mockResolvedValue(LIST.filter((t) => t.id !== 20));
    await useThreads.getState().load(1);
    const s = useThreads.getState();
    expect(s.follow).toBeNull();
    expect(s.props).toBeNull();
  });

  it("an older answer does not replace a newer one", async () => {
    let first!: (v: ArticleThread[]) => void;
    inv.mockImplementationOnce(() => new Promise((r) => (first = r)));
    inv.mockResolvedValueOnce(afterDelete);
    const a = useThreads.getState().load(1);
    await useThreads.getState().load(1);
    first(LIST);
    await a;
    expect(useThreads.getState().get(1)[0].beads).toHaveLength(3);
  });
});

describe("authoring", () => {
  it("a drag starts an article with the next free title and selects its bead", async () => {
    const created = thread(30, "Article 3", [bead(31, 2)]);
    inv.mockImplementation(async (cmd: string) => (cmd === "threads_list" ? [...LIST, created] : { rev: 2 }));
    await useThreads.getState().start(1, 2, [50, 100, 250, 400]);
    expect(inv).toHaveBeenCalledWith("thread_start", { doc: 1, title: "Article 3", page: 2, rect: [50, 100, 250, 400] });
    expect(useThreads.getState().selected).toEqual({ doc: 1, thread: 30, bead: 31 });
  });

  it("a bead added to a thread is selected; Delete of the bead leaves nothing selected", async () => {
    const grown = LIST.map((t) => (t.id === 20 ? { ...t, beads: [...t.beads, bead(22, 4)] } : t));
    inv.mockImplementation(async (cmd: string) => (cmd === "threads_list" ? grown : { rev: 3 }));
    await useThreads.getState().addBead(1, 20, 21, 4, [10, 10, 100, 100]);
    expect(inv).toHaveBeenCalledWith("thread_bead_add", { doc: 1, thread: 20, after: 21, page: 4, rect: [10, 10, 100, 100] });
    expect(useThreads.getState().selected).toEqual({ doc: 1, thread: 20, bead: 22 });
    inv.mockImplementation(async (cmd: string) => (cmd === "threads_list" ? LIST : { rev: 4 }));
    await useThreads.getState().removeBead(1, 20, 22);
    expect(useThreads.getState().selected).toBeNull();
  });

  it("the new bead is selected even when an edit's own read was overtaken by the one the revision change starts", async () => {
    const created = thread(30, "Article 3", [bead(31, 2)]);
    const answers: ((v: ArticleThread[]) => void)[] = [];
    inv.mockImplementation((cmd: string) =>
      cmd === "threads_list" ? new Promise<ArticleThread[]>((r) => answers.push(r)) : Promise.resolve({ rev: 2 }),
    );
    const started = useThreads.getState().start(1, 2, [50, 100, 250, 400]);
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    // doc.rev changed: the host asks again while the edit's own read is still out
    void useThreads.getState().load(1);
    await vi.waitFor(() => expect(answers).toHaveLength(2));
    // the overtaken read lands first and finds the list stale; the edit must wait for the newer one
    answers[0](LIST);
    await new Promise((r) => setTimeout(r, 0));
    answers[1]([...LIST, created]);
    await started;
    expect(useThreads.getState().selected).toEqual({ doc: 1, thread: 30, bead: 31 });
  });

  it("finishing opens the Properties of the article that was being drawn", () => {
    useThreads.setState({ tool: true, selected: { doc: 1, thread: 10, bead: 12 } });
    useThreads.getState().finish();
    expect(useThreads.getState().props).toEqual({ doc: 1, thread: 10 });
    expect(useThreads.getState().selected).toBeNull();
    expect(useThreads.getState().tool).toBe(true);
  });

  it("an engine error is kept for the toast and the list is not touched", async () => {
    inv.mockRejectedValue("This article is damaged");
    await useThreads.getState().removeThread(1, 10);
    expect(useThreads.getState().get(1)).toBe(LIST);
  });
});

describe("helpers", () => {
  it("numbers beads as article-bead", () => {
    expect(beadNumber(LIST, 10, 13)).toBe("1-3");
    expect(beadNumber(LIST, 20, 21)).toBe("2-1");
    expect(beadNumber(LIST, 20, 99)).toBe("");
  });

  it("fits the bead width, clamped, and uses the height for a turned view", () => {
    expect(beadZoom([0, 0, 200, 50], 0, 548)).toBeCloseTo(2.5);
    expect(beadZoom([0, 0, 200, 50], 1, 548)).toBeCloseTo(10);
    expect(beadZoom([0, 0, 1, 50], 0, 5000)).toBe(16);
  });

  it("reconcile keeps a bead, else the one now at its place", () => {
    expect(reconcile(LIST, 10, 13, 0)).toBe(13);
    expect(reconcile(LIST, 10, 99, 9)).toBe(14);
    expect(reconcile(LIST, 99, 1, 0)).toBeNull();
  });

  it("titles are never reused while taken", () => {
    expect(nextTitle([])).toBe("Article 1");
    expect(nextTitle(LIST)).toBe("Article 3");
    expect(nextTitle([thread(1, "Article 2", [bead(2, 0)])])).toBe("Article 3");
  });
});

describe("beads and the other page tools", () => {
  const idle = { redact: false, annot: "select", forms: "select", text: "select", link: "select" };

  it("outlines never take the pointer: Show beads gives only the number tag, reading the whole bead", () => {
    expect(beadPointer({ tool: false, follow: false, show: true, other: false })).toBe("tag");
    expect(beadPointer({ tool: false, follow: true, show: true, other: false })).toBe("all");
    expect(beadPointer({ tool: false, follow: true, show: false, other: false })).toBe("all");
    expect(beadPointer({ tool: false, follow: false, show: false, other: false })).toBe("none");
  });

  it("a tool in the hand keeps the pointer from the beads, tag included", () => {
    expect(beadPointer({ tool: false, follow: true, show: true, other: true })).toBe("none");
    expect(beadPointer({ tool: false, follow: false, show: true, other: true })).toBe("none");
    // the Article tool itself edits them
    expect(beadPointer({ tool: true, follow: false, show: false, other: false })).toBe("edit");
  });

  it("every page tool counts as in the hand: mark text, comments, forms, text edit, links", () => {
    expect(otherToolHeld(idle)).toBe(false);
    expect(otherToolHeld({ ...idle, redact: true })).toBe(true);
    expect(otherToolHeld({ ...idle, annot: "pen" })).toBe(true);
    expect(otherToolHeld({ ...idle, forms: "text" })).toBe(true);
    expect(otherToolHeld({ ...idle, text: "text" })).toBe(true);
    expect(otherToolHeld({ ...idle, link: "draw" })).toBe(true);
  });

  it("the reading keys belong to the page, not to a focused button or list", () => {
    expect(readsKeys({ body: true, inViewer: false })).toBe(true);
    expect(readsKeys({ body: false, inViewer: true })).toBe(true);
    expect(readsKeys({ body: false, inViewer: false })).toBe(false);
  });
});

describe("article properties", () => {
  it("opens with the listed name as the title and counts that as unchanged", () => {
    const t = thread(10, "Article 2", [bead(11, 0)]);
    t.info.title = "";
    const open = propsDraft(t);
    expect(open.title).toBe("Article 2");
    expect(sameInfo(open, propsDraft(t))).toBe(true);
    expect(sameInfo({ ...open, author: "A" }, open)).toBe(false);
    expect(sameInfo({ ...open, title: "Story" }, open)).toBe(false);
  });

  it("an untitled article stays untitled when only another field is edited", () => {
    const t = thread(10, "Article 2", [bead(11, 0)]);
    t.info.title = "";
    const open = propsDraft(t);
    expect(propsChange(t, open)).toBeNull();
    expect(propsChange(t, { ...open, author: "A" })).toEqual({ title: "", subject: "", author: "A", keywords: "" });
    expect(propsChange(t, { ...open, title: "Story" })?.title).toBe("Story");
  });

  it("a titled article is sent as edited, and clearing its title is an edit", () => {
    const t = thread(10, "Story", [bead(11, 0)]);
    const open = propsDraft(t);
    expect(propsChange(t, open)).toBeNull();
    expect(propsChange(t, { ...open, keywords: "k" })).toEqual({ title: "Story", subject: "", author: "", keywords: "k" });
    expect(propsChange(t, { ...open, title: "" })?.title).toBe("");
  });
});

import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
    clear: () => m.clear(),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({
  editImage: vi.fn(),
  editText: vi.fn(),
  addText: vi.fn(),
  setLeaveGuard: vi.fn(),
  save: vi.fn(),
  undo: vi.fn(),
  redo: vi.fn(),
  applyState: vi.fn(),
}));
vi.mock("./api", () => ({ imageBlocks: vi.fn(), pickSource: vi.fn(), textBlocks: vi.fn() }));

import { useStore } from "../store";
import * as api from "./api";
import type { ImageBlock } from "./api";
import { editImage } from "./edit";
import {
  CLICK_BOX,
  MIN_SIDE,
  clampRect,
  closestImage,
  codecName,
  colorName,
  dims,
  dragHandle,
  expectedBox,
  imageSummary,
  mapRect,
  moveRect,
  placement,
  ppi,
  useImageEdit,
} from "./imageEdit";
import { useTextEdit } from "./textEdit";

const image = (over: Partial<ImageBlock> = {}): ImageBlock => ({
  index: 0,
  kind: "xObject",
  rect: [100, 100, 300, 200],
  fullRect: [100, 100, 300, 200],
  matrix: [200, 0, 0, 100, 100, 692],
  width: 800,
  height: 400,
  colorSpace: "DeviceRGB",
  bitsPerComponent: 8,
  filter: ["DCTDecode"],
  masked: false,
  name: "Im1",
  xref: 12,
  editable: true,
  reason: null,
  ...over,
});

describe("helpers", () => {
  it("drags a corner and keeps the proportions, the opposite corner staying put", () => {
    const r: [number, number, number, number] = [0, 0, 100, 50];
    expect(dragHandle(r, "se", [50, 0])).toEqual([0, 0, 150, 75]);
    // The side that moved further decides.
    expect(dragHandle(r, "se", [0, 50])).toEqual([0, 0, 200, 100]);
    expect(dragHandle(r, "nw", [-20, 0])).toEqual([-20, -10, 100, 50]);
    expect(dragHandle(r, "ne", [20, 0])).toEqual([0, -10, 120, 50]);
    expect(dragHandle(r, "sw", [-20, 0])).toEqual([-20, 0, 100, 60]);
  });

  it("drags a corner freely when asked", () => {
    expect(dragHandle([0, 0, 100, 50], "se", [50, 0], true)).toEqual([0, 0, 150, 50]);
  });

  it("drags a side one way only", () => {
    const r: [number, number, number, number] = [10, 10, 110, 60];
    expect(dragHandle(r, "e", [30, 99])).toEqual([10, 10, 140, 60]);
    expect(dragHandle(r, "n", [99, -20])).toEqual([10, -10, 110, 60]);
    expect(dragHandle(r, "w", [-5, 0])).toEqual([5, 10, 110, 60]);
  });

  it("never makes a side shorter than the smallest side", () => {
    const r: [number, number, number, number] = [0, 0, 100, 50];
    expect(dragHandle(r, "e", [-500, 0])).toEqual([0, 0, MIN_SIDE, 50]);
    expect(dragHandle(r, "w", [500, 0])).toEqual([100 - MIN_SIDE, 0, 100, 50]);
    const c = dragHandle(r, "se", [-500, -500]);
    expect(c[2] - c[0]).toBeGreaterThanOrEqual(MIN_SIDE - 1e-9);
    expect(c[3] - c[1]).toBeGreaterThanOrEqual(MIN_SIDE / 2 - 1e-9);
    // ... and a corner keeps its proportions down there.
    expect((c[2] - c[0]) / (c[3] - c[1])).toBeCloseTo(2);
  });

  it("clamps a rectangle inside another", () => {
    expect(clampRect([-5, 10, 700, 900], [0, 0, 612, 792])).toEqual([0, 10, 612, 792]);
    expect(moveRect([1, 2, 3, 4], [10, 20])).toEqual([11, 22, 13, 24]);
  });

  it("places an image by a drag, or in a square for a click", () => {
    const page: [number, number] = [612, 792];
    expect(placement([300, 400], [100, 200], page)).toEqual([100, 200, 300, 400]);
    expect(placement([50, 60], [52, 61], page)).toEqual([50, 60, 50 + CLICK_BOX, 60 + CLICK_BOX]);
    // A click near the edge keeps the square on the page.
    expect(placement([600, 780], [600, 780], page)).toEqual([612 - CLICK_BOX, 792 - CLICK_BOX, 612, 792]);
    // A drag that is thin one way is a click.
    expect(placement([10, 10], [400, 12], page)).toEqual([10, 10, 10 + CLICK_BOX, 10 + CLICK_BOX]);
    // A page smaller than the square.
    expect(placement([5, 5], [5, 5], [100, 120])).toEqual([0, 5, 100, 105]);
  });

  it("knows where an image's box ends up after a change", () => {
    const b = image();
    expect(expectedBox(b, { op: "move", dx: 10, dy: -5 })).toEqual([110, 95, 310, 195]);
    expect(expectedBox(b, { op: "delete" })).toBeNull();
    expect(expectedBox(b, { op: "flip", horizontal: true })).toEqual(b.fullRect);
    expect(expectedBox(b, { op: "rotate", quarterTurns: 2 })).toEqual(b.fullRect);
    // A quarter turn about the centre (200, 150): 200 × 100 becomes 100 × 200.
    expect(expectedBox(b, { op: "rotate", quarterTurns: 1 })).toEqual([150, 50, 250, 250]);
    expect(expectedBox(b, { op: "rotate", quarterTurns: -1 })).toEqual([150, 50, 250, 250]);
    expect(expectedBox(b, { op: "resize", rect: [0, 0, 400, 400], keepAspect: false })).toEqual([0, 0, 400, 400]);
    // Proportions kept: as large as fits, from the top left.
    expect(expectedBox(b, { op: "resize", rect: [0, 0, 400, 400], keepAspect: true })).toEqual([0, 0, 400, 200]);
    expect(expectedBox(b, { op: "crop", rect: [120, 110, 400, 190], pixels: true })).toEqual([120, 110, 300, 190]);
    expect(expectedBox(b, { op: "crop", rect: [120, 110, 200, 190], pixels: false })).toEqual(b.fullRect);
    expect(expectedBox(b, { op: "add", file: "t", rect: [1, 2, 3, 4] })).toEqual([1, 2, 3, 4]);
  });

  it("finds the image a change produced, and not another one", () => {
    const here = image({ index: 2, fullRect: [110, 95, 310, 195], rect: [110, 95, 310, 195] });
    const far = image({ index: 3, fullRect: [100, 600, 300, 700], rect: [100, 600, 300, 700] });
    expect(closestImage([110, 95, 310, 195], [far, here])?.index).toBe(2);
    expect(closestImage([110, 95, 310, 195], [far])).toBeNull();
    expect(closestImage([110, 95, 310, 195], [image({ index: 5, editable: false })])).toBeNull();
  });

  it("maps the visible part of a cropped image when its box is resized, and dims what a crop hides", () => {
    // The right half of a 200 wide box, the box doubled in width.
    expect(mapRect([200, 100, 300, 200], [100, 100, 300, 200], [100, 100, 500, 200])).toEqual([300, 100, 500, 200]);
    const d = dims([0, 0, 100, 100], [20, 30, 80, 70]);
    expect(d).toEqual([
      [0, 0, 100, 30],
      [0, 70, 100, 100],
      [0, 30, 20, 70],
      [80, 30, 100, 70],
    ]);
  });

  it("says what an image is in a person's words", () => {
    expect(ppi(image())).toBeCloseTo(288); // 800 px over 200 pt
    expect(ppi(image({ matrix: [0, 100, -200, 0, 0, 0] }))).toBeCloseTo(576); // turned: 800 px over 100 pt
    expect(imageSummary(image())).toBe("800 × 400 px · 288 ppi");
    expect(codecName(["DCTDecode"])).toBe("JPEG");
    expect(codecName(["ASCII85Decode", "DCTDecode"])).toBe("JPEG");
    expect(codecName(["FlateDecode"])).toBe("Flate");
    expect(codecName([])).toBe("Uncompressed");
    expect(colorName("DeviceRGB")).toBe("RGB");
    expect(colorName("ICCBased")).toBe("ICC colour");
    expect(colorName("Weird")).toBe("Weird");
  });
});

describe("store", () => {
  const list = (images: ImageBlock[], rev = 1) => ({ rev, images });
  const ok = () => ({ kind: "ok" as const, value: {} as never });
  const st = () => useImageEdit.getState().docs[1];
  const bump = (rev: number) => useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev } } });

  beforeEach(() => {
    vi.clearAllMocks();
    useTextEdit.setState({ open: true, tool: "select", docs: {} });
    useImageEdit.setState({ docs: {} });
    useStore.setState({
      docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 0, pageCount: 2, rev: 1, sizes: [[612, 792]] } as never },
      order: [1],
      active: 1,
    });
  });

  it("lists a page once per document generation", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    await useImageEdit.getState().load(1, 0);
    await useImageEdit.getState().load(1, 0);
    expect(api.imageBlocks).toHaveBeenCalledTimes(1);
    bump(2);
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()], 2));
    await useImageEdit.getState().load(1, 0);
    expect(api.imageBlocks).toHaveBeenCalledTimes(2);
  });

  it("moves the selected image as one change and follows it", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValueOnce(list([image({ index: 0 }), image({ index: 1, fullRect: [100, 500, 300, 600], rect: [100, 500, 300, 600] })]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(editImage).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ index: 0, fullRect: [110, 90, 310, 190], rect: [110, 90, 310, 190] })], 2));
    expect(await ie.move(1, [10, -10])).toBe(true);
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, 0, { op: "move", dx: 10, dy: -10 });
    expect(st().selected).toEqual({ page: 0, index: 0 });
  });

  it("deselects after a delete", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValueOnce(list([image()]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(editImage).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([], 2));
    expect(await ie.remove(1)).toBe(true);
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, 0, { op: "delete" });
    expect(st().selected).toBeNull();
  });

  it("does nothing without a selection", async () => {
    expect(await useImageEdit.getState().move(1, [1, 1])).toBe(false);
    expect(editImage).not.toHaveBeenCalled();
  });

  it("tells the person when the engine refuses, and keeps the selection", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(editImage).mockResolvedValue({ kind: "error", message: "this image is drawn inside a text object" });
    expect(await ie.rotate(1, 1)).toBe(false);
    expect(st().notice).toContain("inside a text object");
    expect(st().selected).toEqual({ page: 0, index: 0 });
  });

  it("finds the image again after a stale change, and says it was not applied", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValueOnce(list([image({ index: 0 })]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(editImage).mockResolvedValue({ kind: "stale" });
    // Another image was added before it: ours is index 1 now.
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ index: 0, fullRect: [10, 10, 40, 40], rect: [10, 10, 40, 40] }), image({ index: 1 })], 2));
    expect(await ie.flip(1, true)).toBe(false);
    expect(st().selected).toEqual({ page: 0, index: 1 });
    expect(st().notice).toMatch(/not applied/);
  });

  it("follows the selected image to its new index after undo or redo, and drops it when it is gone", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ index: 0, fullRect: [10, 10, 40, 40], rect: [10, 10, 40, 40] }), image({ index: 1 })]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 1 });
    bump(2);
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ index: 0 })], 2));
    await ie.load(1, 0);
    expect(st().selected).toEqual({ page: 0, index: 0 });
    bump(3);
    vi.mocked(api.imageBlocks).mockResolvedValue(list([], 3));
    await ie.load(1, 0);
    expect(st().selected).toBeNull();
  });

  it("replaces with the file chosen, and does nothing when none is", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(api.pickSource).mockResolvedValueOnce([]);
    expect(await ie.replace(1)).toBe(false);
    expect(editImage).not.toHaveBeenCalled();
    vi.mocked(api.pickSource).mockResolvedValueOnce([{ token: "tok", path: "C:\\p\\logo.png" }]);
    vi.mocked(editImage).mockResolvedValue(ok());
    expect(await ie.replace(1)).toBe(true);
    expect(api.pickSource).toHaveBeenCalledWith("images", false);
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, 0, { op: "replace", file: "tok" });
  });

  it("places a chosen file where the page is clicked and selects the new image", async () => {
    vi.mocked(api.pickSource).mockResolvedValue([{ token: "tok", path: "C:\\p\\photo.jpg" }]);
    const ie = useImageEdit.getState();
    await ie.startPlacing(1);
    expect(st().placing).toEqual({ file: "tok", name: "photo.jpg" });
    vi.mocked(editImage).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ index: 4, fullRect: [100, 100, 280, 280], rect: [100, 100, 280, 280] })], 2));
    expect(await ie.place(1, 0, [100, 100, 280, 280])).toBe(true);
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, null, { op: "add", file: "tok", rect: [100, 100, 280, 280], keepAspect: true });
    expect(st().placing).toBeNull();
    expect(st().selected).toEqual({ page: 0, index: 4 });
  });

  it("keeps the chosen file when the engine refuses, and gives it up for the text tool", async () => {
    vi.mocked(api.pickSource).mockResolvedValue([{ token: "tok", path: "C:\\p\\bad.png" }]);
    const ie = useImageEdit.getState();
    await ie.startPlacing(1);
    vi.mocked(editImage).mockResolvedValue({ kind: "error", message: "no such image" });
    expect(await ie.place(1, 0, [0, 0, 50, 50])).toBe(false);
    expect(st().placing).not.toBeNull();
    expect(st().notice).toContain("no such image");
    useTextEdit.getState().setTool("text");
    expect(st().placing).toBeNull();
  });

  it("retries a placement once when the document moved on", async () => {
    vi.mocked(api.pickSource).mockResolvedValue([{ token: "tok", path: "C:\\p\\photo.jpg" }]);
    const ie = useImageEdit.getState();
    await ie.startPlacing(1);
    vi.mocked(editImage).mockResolvedValueOnce({ kind: "stale" }).mockResolvedValueOnce(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([], 2));
    expect(await ie.place(1, 0, [0, 0, 50, 50])).toBe(true);
    expect(editImage).toHaveBeenCalledTimes(2);
  });

  it("crops to the rectangle set, as a clip or by pixels, and skips a crop that cuts nothing", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    vi.mocked(editImage).mockResolvedValue(ok());
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    ie.startCrop(1);
    expect(st().crop).toEqual({ rect: [100, 100, 300, 200], pixels: false });
    expect(await ie.applyCrop(1)).toBe(true);
    expect(editImage).not.toHaveBeenCalled();
    expect(st().crop).toBeNull();

    ie.startCrop(1);
    ie.setCrop(1, [150, 100, 400, 180]);
    ie.setCropPixels(1, true);
    expect(await ie.applyCrop(1)).toBe(true);
    // Kept inside the image's box.
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, 0, { op: "crop", rect: [150, 100, 300, 180], pixels: true });
    expect(st().crop).toBeNull();
  });

  it("does not crop a locked image", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ editable: false, reason: "inside a form" })]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    ie.startCrop(1);
    expect(st().crop).toBeNull();
  });

  it("lets go of the image when a text block is taken up, and of everything when Edit closes", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    vi.mocked(api.pickSource).mockResolvedValue([{ token: "tok", path: "C:\\p\\photo.jpg" }]);
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    useTextEdit.setState({
      docs: { 1: { pages: {}, selected: { page: 0, index: 0 }, draft: null, adding: null, notice: null, failed: false } },
    });
    expect(st().selected).toBeNull();

    ie.select(1, { page: 0, index: 0 });
    await ie.startPlacing(1);
    useTextEdit.getState().setOpen(false);
    expect(st().selected).toBeNull();
    expect(st().placing).toBeNull();
  });

  it("forgets a closed tab", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    await useImageEdit.getState().load(1, 0);
    expect(st()).toBeDefined();
    useStore.setState({ docs: {}, order: [], active: null });
    expect(st()).toBeUndefined();
  });
});

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
  editImages: vi.fn(),
  pasteImages: vi.fn(),
  moveImagesToPage: vi.fn(),
  editText: vi.fn(),
  addText: vi.fn(),
  setLeaveGuard: vi.fn(),
  save: vi.fn(),
  undo: vi.fn(),
  redo: vi.fn(),
  applyState: vi.fn(),
}));
vi.mock("./api", () => ({ imageBlocks: vi.fn(), imageCopy: vi.fn(), imageClipInfo: vi.fn(), pickSource: vi.fn(), textBlocks: vi.fn() }));

import { useStore } from "../store";
import * as api from "./api";
import type { ImageBlock } from "./api";
import { editImage, editImages, moveImagesToPage, pasteImages } from "./edit";
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
  alignDeltas,
  dragTurn,
  dropPoint,
  groupBox,
  pastePoint,
  isQuarter,
  marqueeImages,
  normAngle,
  quadTouches,
  togglePick,
  turnQuad,
  turnTo,
  useImageEdit,
} from "./imageEdit";
import { useTextEdit } from "./textEdit";

const image = (over: Partial<ImageBlock> = {}): ImageBlock => ({
  index: 0,
  kind: "xObject",
  rect: [100, 100, 300, 200],
  fullRect: [100, 100, 300, 200],
  matrix: [200, 0, 0, 100, 100, 692],
  angle: 0,
  quad: [100, 200, 300, 200, 300, 100, 100, 100],
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
  const list = (images: ImageBlock[], rev = 1) => ({ rev, images, locked: null, signed: false, copyLocked: null });
  const ok = () => ({ kind: "ok" as const, value: {} as never });
  const st = () => useImageEdit.getState().docs[1];
  const bump = (rev: number) => useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev } } });

  beforeEach(() => {
    vi.clearAllMocks();
    useTextEdit.setState({ open: true, tool: "select", section: "text", docs: {} });
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
      docs: { 1: { pages: {}, selected: { page: 0, index: 0 }, draft: null, selection: null, adding: null, notice: null, failed: false } },
    });
    expect(st().selected).toBeNull();

    ie.select(1, { page: 0, index: 0 });
    await ie.startPlacing(1);
    useTextEdit.getState().setOpen(false);
    expect(st().selected).toBeNull();
    expect(st().placing).toBeNull();
  });

  it("lets go of everything when Edit moves to Links or Bookmarks", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    vi.mocked(api.pickSource).mockResolvedValue([{ token: "tok", path: "C:\\p\\photo.jpg" }]);
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    ie.startCrop(1);
    expect(st().crop).not.toBeNull();
    useTextEdit.getState().setSection("links");
    expect(st().selected).toBeNull();
    expect(st().crop).toBeNull();

    useTextEdit.getState().setSection("text");
    await ie.startPlacing(1);
    expect(st().placing).not.toBeNull();
    useTextEdit.getState().setSection("bookmarks");
    expect(st().placing).toBeNull();
    useTextEdit.getState().setSection("text");
  });

  it("keeps the selection on the image a change moved, when the listing it triggered lands last", async () => {
    // The moved image jumps to where a neighbour was, and the neighbour takes its old place.
    const moved = image({ index: 0, fullRect: [100, 600, 300, 700], rect: [100, 600, 300, 700] });
    const neighbour = image({ index: 1, fullRect: [100, 100, 300, 200], rect: [100, 100, 300, 200] });
    vi.mocked(api.imageBlocks).mockResolvedValueOnce(list([image({ index: 0 }), image({ index: 1, fullRect: [100, 500, 300, 600], rect: [100, 500, 300, 600] })]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });

    // The document's generation bumps with the change; the page asks for the list again (the layer's
    // effect), and that answer is the slow one.
    let slow!: (v: ReturnType<typeof list>) => void;
    let reloading: Promise<void> = Promise.resolve();
    vi.mocked(editImage).mockImplementation(async () => {
      bump(2);
      vi.mocked(api.imageBlocks).mockImplementationOnce(() => new Promise((res) => (slow = res)));
      reloading = useImageEdit.getState().load(1, 0);
      vi.mocked(api.imageBlocks).mockResolvedValueOnce(list([moved, neighbour], 2));
      return ok();
    });
    expect(await ie.move(1, [0, 500])).toBe(true);
    expect(st().selected).toEqual({ page: 0, index: 0 });
    slow(list([moved, neighbour], 2));
    await reloading;
    expect(st().selected).toEqual({ page: 0, index: 0 });
  });

  it("applies a typed size to the picture it was typed for, though another is selected by then", async () => {
    const a = image({ index: 0 });
    const b = image({ index: 1, fullRect: [100, 500, 300, 600], rect: [100, 500, 300, 600] });
    vi.mocked(api.imageBlocks).mockResolvedValueOnce(list([a, b]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(editImage).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image({ index: 0, fullRect: [100, 100, 400, 250], rect: [100, 100, 400, 250] }), b], 2));
    // The field commits on blur: the click that blurs it has selected the other picture already.
    ie.select(1, { page: 0, index: 1 });
    const sent = ie.resizeTo(1, [100, 100, 400, 250], false, { page: 0, index: 0 });
    expect(await sent).toBe(true);
    expect(editImage).toHaveBeenCalledTimes(1);
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, 0, { op: "resize", rect: [100, 100, 400, 250], keepAspect: false });
    // The picture that was clicked keeps the selection, and is not touched.
    expect(st().selected).toEqual({ page: 0, index: 1 });
  });

  it("does not select a picture again when the click that committed its size cleared the selection", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    vi.mocked(editImage).mockResolvedValue(ok());
    ie.select(1, null);
    expect(await ie.resizeTo(1, [100, 100, 400, 250], false, { page: 0, index: 0 })).toBe(true);
    expect(st().selected).toBeNull();
  });

  it("lets go of the text block when a picture is taken from the rail's list", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    await useImageEdit.getState().load(1, 0);
    const original = useTextEdit.getState().select;
    const spy = vi.fn().mockResolvedValue(undefined);
    useTextEdit.setState({
      select: spy,
      docs: { 1: { pages: {}, selected: { page: 0, index: 2 }, draft: null, adding: null, notice: null, failed: false } as never },
    });
    try {
      useImageEdit.getState().select(1, { page: 0, index: 0 });
      expect(spy).toHaveBeenCalledWith(1, null);
      // Letting go of the picture does not touch the text.
      spy.mockClear();
      useImageEdit.getState().select(1, null);
      expect(spy).not.toHaveBeenCalled();
    } finally {
      useTextEdit.setState({ select: original });
    }
  });

  it("forgets a closed tab", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list([image()]));
    await useImageEdit.getState().load(1, 0);
    expect(st()).toBeDefined();
    useStore.setState({ docs: {}, order: [], active: null });
    expect(st()).toBeUndefined();
  });
});

describe("several images", () => {
  const at = (index: number, r: [number, number, number, number], over: Partial<ImageBlock> = {}) =>
    image({ index, rect: r, fullRect: r, quad: [r[0], r[3], r[2], r[3], r[2], r[1], r[0], r[1]], ...over });

  it("toggles a pick within one page, the last pick being the anchor", () => {
    const a = { selected: { page: 0, index: 1 }, extra: [0] };
    expect(togglePick(a, { page: 0, index: 2 })).toEqual([
      { page: 0, index: 0 },
      { page: 0, index: 1 },
      { page: 0, index: 2 },
    ]);
    expect(togglePick(a, { page: 0, index: 0 })).toEqual([{ page: 0, index: 1 }]);
    // Another page starts a new pick: a selection is one page's at a time.
    expect(togglePick(a, { page: 1, index: 0 })).toEqual([{ page: 1, index: 0 }]);
    expect(togglePick({ selected: null, extra: [] }, { page: 0, index: 3 })).toEqual([{ page: 0, index: 3 }]);
  });

  it("selects the editable images a marquee touches or contains", () => {
    const images = [at(0, [0, 0, 50, 50]), at(1, [100, 0, 150, 50]), at(2, [40, 40, 120, 120], { editable: false }), at(3, [200, 200, 300, 300])];
    // touches 0, contains nothing else; 2 is locked and never picked; 1 is only reached with a wider box
    expect(marqueeImages(images, [45, 45, 60, 60])).toEqual([0]);
    expect(marqueeImages(images, [10, 10, 140, 40])).toEqual([0, 1]);
    // Inside an image's own box still touches it; edges that only meet do not.
    expect(marqueeImages(images, [210, 210, 220, 220])).toEqual([3]);
    expect(marqueeImages(images, [50, 0, 100, 50])).toEqual([]);
  });

  it("tests a turned image by its outline, not its bounding box", () => {
    // A diamond with corners at the middle of a 100 pt box's sides: the box's corner is empty.
    const quad: ImageBlock["quad"] = [50, 100, 100, 50, 50, 0, 0, 50];
    expect(quadTouches(quad, [0, 0, 10, 10])).toBe(false);
    expect(quadTouches(quad, [40, 40, 60, 60])).toBe(true);
    expect(quadTouches(quad, [90, 90, 120, 120])).toBe(false);
    const d = at(0, [0, 0, 100, 100], { angle: 45, quad });
    expect(marqueeImages([d], [0, 0, 10, 10])).toEqual([]);
    expect(marqueeImages([d], [0, 0, 30, 30])).toEqual([0]);
  });

  it("lines images up with the group's edges and centres", () => {
    const rects: [number, number, number, number][] = [
      [10, 10, 30, 30],
      [50, 40, 100, 60],
    ];
    expect(groupBox(rects)).toEqual([10, 10, 100, 60]);
    expect(alignDeltas(rects, "left")).toEqual([[0, 0], [-40, 0]]);
    expect(alignDeltas(rects, "right")).toEqual([[70, 0], [0, 0]]);
    expect(alignDeltas(rects, "hcenter")).toEqual([[35, 0], [-20, 0]]);
    expect(alignDeltas(rects, "top")).toEqual([[0, 0], [0, -30]]);
    expect(alignDeltas(rects, "bottom")).toEqual([[0, 30], [0, 0]]);
    expect(alignDeltas(rects, "vcenter")).toEqual([[0, 15], [0, -15]]);
  });

  it("works out the turn a typed angle or a handle drag makes", () => {
    expect(normAngle(190)).toBe(-170);
    expect(normAngle(-180)).toBe(180);
    expect(isQuarter(90)).toBe(true);
    expect(isQuarter(-180)).toBe(true);
    expect(isQuarter(30)).toBe(false);
    expect(turnTo(0, 30)).toBe(30);
    expect(turnTo(350, 10)).toBe(20);
    expect(turnTo(30, 30)).toBeNull();
    // Straight up to straight right is a quarter turn clockwise.
    expect(dragTurn([100, 100], [100, 0], [200, 100], false)).toBeCloseTo(90);
    // Snapped to 15: 20 degrees becomes 15.
    expect(dragTurn([0, 0], [0, -100], [Math.sin((20 * Math.PI) / 180) * 100, -Math.cos((20 * Math.PI) / 180) * 100], true)).toBe(15);
  });

  it("turns a quad about a centre, clockwise as seen", () => {
    // y points down: the point straight up from the centre goes to its right.
    const q = turnQuad([0, -10, 0, -10, 0, -10, 0, -10], [0, 0], 90);
    expect(q[0]).toBeCloseTo(10);
    expect(q[1]).toBeCloseTo(0);
  });
});

describe("several images: the store", () => {
  const list = (images: ImageBlock[], rev = 1, locked: string | null = null) => ({ rev, images, locked, signed: false, copyLocked: null });
  const ok = () => ({ kind: "ok" as const, value: {} as never });
  const st = () => useImageEdit.getState().docs[1];
  const box = (index: number, x: number): ImageBlock => image({ index, rect: [x, 100, x + 50, 150], fullRect: [x, 100, x + 50, 150] });
  const three = () => [box(0, 0), box(1, 100), box(2, 200)];

  beforeEach(() => {
    vi.clearAllMocks();
    useTextEdit.setState({ open: true, tool: "select", section: "text", docs: {} });
    useImageEdit.setState({ docs: {} });
    useStore.setState({
      docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 0, pageCount: 2, rev: 1, sizes: [[612, 792]] } as never },
      order: [1],
      active: 1,
    });
  });

  const picked = async (indices: number[]) => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list(three()));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.pickMany(1, 0, indices, false);
    return ie;
  };

  it("sends one change for the whole group and keeps all of them picked", async () => {
    const ie = await picked([0, 2]);
    expect(st().selected).toEqual({ page: 0, index: 2 });
    expect(st().extra).toEqual([0]);
    vi.mocked(editImages).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 10), box(1, 100), box(2, 210)], 2));
    expect(await ie.move(1, [10, 0])).toBe(true);
    expect(editImages).toHaveBeenCalledTimes(1);
    expect(editImages).toHaveBeenCalledWith(1, 1, 0, [
      { index: 0, change: { op: "move", dx: 10, dy: 0 } },
      { index: 2, change: { op: "move", dx: 10, dy: 0 } },
    ]);
    expect(editImage).not.toHaveBeenCalled();
    expect(st().selected).toEqual({ page: 0, index: 2 });
    expect(st().extra).toEqual([0]);
  });

  it("deletes the group in one step and picks nothing after", async () => {
    const ie = await picked([0, 1]);
    vi.mocked(editImages).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 200)], 2));
    expect(await ie.remove(1)).toBe(true);
    expect(editImages).toHaveBeenCalledWith(1, 1, 0, [
      { index: 0, change: { op: "delete" } },
      { index: 1, change: { op: "delete" } },
    ]);
    expect(st().selected).toBeNull();
    expect(st().extra).toEqual([]);
  });

  it("aligns: one move per image that has to move", async () => {
    const ie = await picked([0, 1, 2]);
    vi.mocked(editImages).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 0), box(1, 0), box(2, 0)], 2));
    expect(await ie.align(1, "left")).toBe(true);
    expect(editImages).toHaveBeenCalledWith(1, 1, 0, [
      { index: 1, change: { op: "move", dx: -100, dy: 0 } },
      { index: 2, change: { op: "move", dx: -200, dy: 0 } },
    ]);
  });

  it("turns the group by an angle, each about its own centre", async () => {
    const ie = await picked([0, 1]);
    vi.mocked(editImages).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockResolvedValue(list(three(), 2));
    await ie.turn(1, 30);
    expect(editImages).toHaveBeenCalledWith(1, 1, 0, [
      { index: 0, change: { op: "angle", degrees: 30 } },
      { index: 1, change: { op: "angle", degrees: 30 } },
    ]);
  });

  it("a single pick still goes the one-image way", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list(three()));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 1 });
    vi.mocked(editImage).mockResolvedValue(ok());
    await ie.turn(1, 30);
    expect(editImage).toHaveBeenCalledWith(1, 1, 0, 1, { op: "angle", degrees: 30 });
    expect(editImages).not.toHaveBeenCalled();
  });

  it("toggles picks with a click and a click elsewhere starts again", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list(three()));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.select(1, { page: 0, index: 0 });
    ie.toggle(1, { page: 0, index: 2 });
    expect(st().selected).toEqual({ page: 0, index: 2 });
    expect(st().extra).toEqual([0]);
    ie.toggle(1, { page: 0, index: 0 });
    expect(st().selected).toEqual({ page: 0, index: 2 });
    expect(st().extra).toEqual([]);
    ie.select(1, { page: 0, index: 1 });
    expect(st().extra).toEqual([]);
  });

  it("a marquee with shift adds to the picks", async () => {
    const ie = await picked([0]);
    ie.pickMany(1, 0, [2], true);
    expect(st().selected).toEqual({ page: 0, index: 2 });
    expect(st().extra).toEqual([0]);
    ie.pickMany(1, 0, [], false);
    expect(st().selected).toBeNull();
  });

  it("follows every pick by position after undo or redo, dropping the ones that are gone", async () => {
    await picked([0, 2]);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 2 } } });
    // The first image went away: the others are 0 and 1 now.
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(1, 100), box(2, 200)].map((b, i) => ({ ...b, index: i })), 2));
    await useImageEdit.getState().load(1, 0);
    expect(st().selected).toEqual({ page: 0, index: 1 });
    expect(st().extra).toEqual([]);
  });

  it("refuses a group change in a certified document before sending it", async () => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list(three(), 1, "This document is certified."));
    const ie = useImageEdit.getState();
    await ie.load(1, 0);
    ie.pickMany(1, 0, [0, 1], false);
    expect(await ie.move(1, [5, 5])).toBe(false);
    expect(await ie.turn(1, 30)).toBe(false);
    expect(await ie.remove(1)).toBe(false);
    expect(editImages).not.toHaveBeenCalled();
    expect(st().notice).toContain("certified");
    ie.select(1, { page: 0, index: 0 });
    expect(await ie.turn(1, 30)).toBe(false);
    expect(editImage).not.toHaveBeenCalled();
  });
});

describe("copy, paste and move to another page", () => {
  const list = (images: ImageBlock[], rev = 1, extra: Partial<{ locked: string | null; copyLocked: string | null }> = {}) => ({
    rev,
    images,
    locked: null,
    signed: false,
    copyLocked: null,
    ...extra,
  });
  const ok = () => ({ kind: "ok" as const, value: {} as never });
  const st = (doc = 1) => useImageEdit.getState().docs[doc];
  const box = (index: number, x: number, over: Partial<ImageBlock> = {}): ImageBlock =>
    image({ index, rect: [x, 100, x + 50, 150], fullRect: [x, 100, x + 50, 150], ...over });
  const info = { count: 2, rect: [10, 100, 160, 150] as [number, number, number, number], rects: [] };

  beforeEach(() => {
    vi.clearAllMocks();
    useTextEdit.setState({ open: true, tool: "select", section: "text", docs: {} });
    useImageEdit.setState({ docs: {}, clip: null, pastes: {} });
    const doc = (id: number) => ({ id, path: `C:\d\${id}.pdf`, name: `${id}.pdf`, dirty: false, currentPage: 0, pageCount: 3, rev: 1, sizes: [[612, 792]] }) as never;
    useStore.setState({ docs: { 1: doc(1), 2: doc(2) }, order: [1, 2], active: 1 });
  });

  const picked = async (images: ImageBlock[], indices: number[], page = 0, doc = 1, over = {}) => {
    vi.mocked(api.imageBlocks).mockResolvedValue(list(images, 1, over));
    const ie = useImageEdit.getState();
    await ie.load(doc, page);
    ie.pickMany(doc, page, indices, false);
    return ie;
  };

  it("works out where a paste goes and where a dropped group lands", () => {
    expect(pastePoint([10, 100, 160, 150], 0)).toEqual([10, 100]);
    expect(pastePoint([10, 100, 160, 150], 2)).toEqual([42, 132]);
    // The pointer was 5 and 7 inside the group's corner when it was picked up: the corner lands there.
    expect(dropPoint([300, 400], [105, 207], [100, 200])).toEqual([295, 393]);
  });

  it("copies the picked pictures that can be copied, and says what was left out", async () => {
    const ie = await picked([box(0, 0), box(1, 100, { kind: "inline" }), box(2, 200)], [0, 1, 2]);
    vi.mocked(api.imageCopy).mockResolvedValue({ ...info, count: 2 });
    expect(await ie.copy(1)).toBe(true);
    expect(api.imageCopy).toHaveBeenCalledWith(1, 1, 0, [0, 2]);
    expect(useImageEdit.getState().clip?.from).toEqual({ doc: 1, page: 0 });
    expect(st().notice).toContain("1 of the pictures were not copied");
  });

  it("copies nothing from inline pictures only, or from a document that withholds copying", async () => {
    const ie = await picked([box(0, 0, { kind: "inline" })], [0]);
    expect(await ie.copy(1)).toBe(false);
    expect(st().notice).toContain("inline");
    useImageEdit.setState({ docs: {} });
    const again = await picked([box(0, 0)], [0], 0, 1, { copyLocked: "The owner does not allow copying." });
    expect(await again.copy(1)).toBe(false);
    expect(st().notice).toContain("does not allow");
    expect(api.imageCopy).not.toHaveBeenCalled();
  });

  it("pastes into another document at the copied place, and steps the next paste on the same page", async () => {
    useImageEdit.setState({ clip: { info, from: { doc: 1, page: 0 } } });
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 400), box(1, 10), box(2, 70)], 1));
    vi.mocked(pasteImages).mockResolvedValue(ok());
    const ie = useImageEdit.getState();
    expect(await ie.paste(2, 0)).toBe(true);
    expect(pasteImages).toHaveBeenLastCalledWith(2, 1, 0, [10, 100]);
    // The two last pictures drawn are the pasted ones, and they are the picks.
    expect(st(2).selected).toEqual({ page: 0, index: 2 });
    expect(st(2).extra).toEqual([1]);
    expect(await ie.paste(2, 0)).toBe(true);
    expect(pasteImages).toHaveBeenLastCalledWith(2, 1, 0, [26, 116]);
  });

  it("offsets a paste on the page it was copied from, so it is not hidden under the original", async () => {
    useImageEdit.setState({ clip: { info, from: { doc: 1, page: 0 } } });
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 10), box(1, 70), box(2, 26), box(3, 86)], 1));
    vi.mocked(pasteImages).mockResolvedValue(ok());
    await useImageEdit.getState().paste(1, 0);
    expect(pasteImages).toHaveBeenCalledWith(1, 1, 0, [26, 116]);
  });

  it("refuses to paste with nothing copied or into a certified document, before sending anything", async () => {
    vi.mocked(api.imageClipInfo).mockResolvedValue(null);
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 0)], 1));
    expect(await useImageEdit.getState().paste(1, 0)).toBe(false);
    expect(st().notice).toContain("No pictures are copied");
    useImageEdit.setState({ clip: { info, from: null } });
    vi.mocked(api.imageBlocks).mockResolvedValue(list([box(0, 0)], 1, { locked: "This document is certified." }));
    expect(await useImageEdit.getState().paste(1, 0)).toBe(false);
    expect(st().notice).toContain("certified");
    expect(pasteImages).not.toHaveBeenCalled();
  });

  it("moves the picks to another page in one call and picks them there", async () => {
    const ie = await picked([box(0, 0), box(1, 100), box(2, 200)], [0, 2]);
    vi.mocked(moveImagesToPage).mockResolvedValue(ok());
    vi.mocked(api.imageBlocks).mockImplementation(async (_d, page) => list(page === 1 ? [box(0, 5), box(1, 80), box(2, 150)] : [box(0, 100)], 2));
    expect(await ie.transfer(1, 1, [30, 40])).toBe(true);
    expect(moveImagesToPage).toHaveBeenCalledWith(1, 1, 0, [0, 2], 1, [30, 40]);
    expect(st().selected).toEqual({ page: 1, index: 2 });
    expect(st().extra).toEqual([1]);
  });

  it("does not move to the same page, or out of a certified document", async () => {
    const ie = await picked([box(0, 0)], [0]);
    expect(await ie.transfer(1, 0, [0, 0])).toBe(false);
    useImageEdit.setState({ docs: {} });
    const locked = await picked([box(0, 0)], [0], 0, 1, { locked: "This document is certified." });
    expect(await locked.transfer(1, 1, [0, 0])).toBe(false);
    expect(moveImagesToPage).not.toHaveBeenCalled();
  });
});

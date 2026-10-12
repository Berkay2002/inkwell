import { beforeEach, describe, expect, it, vi } from "vitest";

const run = vi.fn();
const combine = vi.fn();

vi.mock("./toolItems", () => ({
  toolItems: () => [
    { id: "protect", onSelect: () => {} },
    { id: "compress", onSelect: () => {} },
  ],
  runTool: (item: { id: string }) => run(item.id),
}));
vi.mock("./combineStore", () => ({ useCombine: { getState: () => ({ open: combine }) } }));

import { openAppTool } from "./aiOpenTool";

describe("open_tool", () => {
  beforeEach(() => {
    run.mockClear();
    combine.mockClear();
  });

  it("runs the app's tool of that id the way the All tools list does", async () => {
    expect(await openAppTool("protect", { id: 1, name: "a.pdf" })).toBe(true);
    expect(run).toHaveBeenCalledWith("protect");
    expect(combine).not.toHaveBeenCalled();
  });

  it("opens Combine, which is not in the All tools list", async () => {
    expect(await openAppTool("merge", null)).toBe(true);
    expect(combine).toHaveBeenCalledTimes(1);
    expect(run).not.toHaveBeenCalled();
  });

  it("does nothing for an id the app does not have", async () => {
    expect(await openAppTool("sign-everything", null)).toBe(false);
    expect(run).not.toHaveBeenCalled();
    expect(combine).not.toHaveBeenCalled();
  });
});

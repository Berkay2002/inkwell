import { describe, expect, it } from "vitest";
import { isVerified, trailSummary, waiting } from "./activity";
import type { Activity, Citation } from "./ai";

const act = (label: string, state: Activity["state"] = "done", id = label): Activity => ({
  id,
  tool: "t",
  class: "read",
  label,
  state,
  detail: null,
});

describe("activity trail", () => {
  it("joins the calls of a turn on one quiet line", () => {
    const s = trailSummary([act('Searched "Ciri"'), act("Read pp. 40-52"), act("Went to p. 41")]);
    expect(s).toBe('Searched "Ciri" · Read pp. 40-52 · Went to p. 41');
  });

  it("collapses repeats, marks refusals and caps a long run", () => {
    expect(trailSummary([act("Read p. 3", "done", "a"), act("Read p. 3", "done", "b")])).toBe("Read p. 3");
    expect(trailSummary([act("Deleted p. 2", "rejected")])).toBe("Deleted p. 2 (rejected)");
    expect(trailSummary([act("x", "failed")])).toBe("x (failed)");
    const many = Array.from({ length: 9 }, (_, i) => act(`Read p. ${i}`));
    expect(trailSummary(many).endsWith("· +3 more")).toBe(true);
  });

  it("lists only the calls that wait for the user", () => {
    expect(waiting([act("a"), act("b", "waiting"), act("c", "running")]).map((a) => a.label)).toEqual(["b"]);
    expect(waiting(undefined)).toEqual([]);
  });

  it("trusts a citation only when its quote was found on a page", () => {
    const c = { n: 1, page: 2, rects: [], quote: "q", chunk: null, exact: true } as Citation;
    expect(isVerified(c)).toBe(true);
    expect(isVerified({ ...c, exact: false })).toBe(false);
    expect(isVerified({ ...c, page: null })).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { addOffer, isVerified, touchedRedaction, trailSummary, waiting } from "./activity";
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

  it("says how far a long job is while it runs", () => {
    const run = (detail: string | null): Activity => ({ ...act("Make a compressed copy (smallest)", "running"), detail });
    expect(trailSummary([run("Compressing: 40%")])).toBe("Make a compressed copy (smallest) (Compressing: 40%)");
    expect(trailSummary([run(null)])).toBe("Make a compressed copy (smallest)");
    // done: the last progress line is not repeated
    expect(trailSummary([{ ...run("Compressing: 100%"), state: "done" }])).toBe("Make a compressed copy (smallest)");
  });

  it("offers each made file once", () => {
    const f = { name: "a (compressed).pdf", path: "C:/x/a (compressed).pdf", token: "t1" };
    const one = addOffer(undefined, f);
    expect(one).toEqual([f]);
    expect(addOffer(one, { ...f, token: "t2" })).toBe(one);
    expect(addOffer(one, { name: "a.txt", path: "C:/x/a.txt", token: "t3" })).toHaveLength(2);
  });

  it("lists only the calls that wait for the user", () => {
    expect(waiting([act("a"), act("b", "waiting"), act("c", "running")]).map((a) => a.label)).toEqual(["b"]);
    expect(waiting(undefined)).toEqual([]);
  });

  it("offers Review in Redact only after a redaction tool really changed something", () => {
    const r = (tool: string, cls: Activity["class"], state: Activity["state"]): Activity => ({ ...act(tool, state), tool, class: cls });
    expect(touchedRedaction([r("redact_find_and_mark", "edit", "done")])).toBe(true);
    expect(touchedRedaction([r("redact_apply", "permanent", "done")])).toBe(true);
    expect(touchedRedaction([r("redact_list_marks", "read", "done")])).toBe(false);
    expect(touchedRedaction([r("redact_find_and_mark", "edit", "failed"), r("redact_apply", "permanent", "rejected")])).toBe(false);
    expect(touchedRedaction([r("add_comment", "edit", "done")])).toBe(false);
    expect(touchedRedaction(undefined)).toBe(false);
  });

  it("trusts a citation only when its quote was found on a page", () => {
    const c = { n: 1, page: 2, rects: [], quote: "q", chunk: null, exact: true } as Citation;
    expect(isVerified(c)).toBe(true);
    expect(isVerified({ ...c, exact: false })).toBe(false);
    expect(isVerified({ ...c, page: null })).toBe(false);
  });
});

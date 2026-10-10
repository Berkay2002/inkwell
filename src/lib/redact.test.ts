import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { applyState } from "./edit";
import { useRedact, type Hit, type Mark } from "./redact";

const inv = vi.mocked(invoke);
const hit = (page: number, text: string): Hit => ({ page, text, quads: [[0, 0, 1, 0, 0, 1, 1, 1]] });
const mark = (page: number, xref: number): Mark => ({ page, xref, quads: [], wholePage: false });
const state = { rev: 1, pageCount: 3, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Mark 1 item", redo: null };
const options = { sanitize: { metadata: true, bookmarks: true }, preview: {}, saveName: "a_Redacted.pdf" };
const r = () => useRedact.getState();

/** Answers each command by name. */
function reply(map: Record<string, unknown>) {
  inv.mockImplementation((cmd: string) =>
    cmd in map ? Promise.resolve(map[cmd]) : Promise.reject(`unexpected ${cmd}`),
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  useRedact.setState({ docs: {} });
});

describe("redact store", () => {
  it("finds, checks all hits, and marks only the checked ones", async () => {
    reply({ redact_find: [hit(0, "a"), hit(1, "b"), hit(2, "c")] });
    await r().find(1, { kind: "pattern", pattern: "email" });
    expect(inv).toHaveBeenCalledWith("redact_find", { doc: 1, query: { kind: "pattern", pattern: "email" } });
    expect(r().get(1).checked).toEqual([0, 1, 2]);

    r().toggleHit(1, 1);
    expect(r().get(1).checked).toEqual([0, 2]);
    reply({ redact_mark: state, redact_list_marks: [mark(0, 9), mark(2, 10)] });
    await r().markChecked(1);
    expect(inv).toHaveBeenCalledWith("redact_mark", { doc: 1, what: { kind: "hits", hits: [hit(0, "a"), hit(2, "c")] } });
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(r().get(1).marks).toHaveLength(2);
    expect(r().get(1).hits).toEqual([]);
  });

  it("unmarks by page and handle", async () => {
    reply({ redact_unmark: state, redact_list_marks: [] });
    await r().unmark(1, [mark(0, 9)]);
    expect(inv).toHaveBeenCalledWith("redact_unmark", { doc: 1, marks: [[0, 9]] });
    expect(r().get(1).marks).toEqual([]);
  });

  it("confirm needs marks, then apply asks the backend to save and shows the report", async () => {
    await r().confirm(1);
    expect(r().get(1).phase).toBe("marking");

    useRedact.setState({ docs: { 1: { ...r().get(1), marks: [mark(0, 9)] } } });
    reply({ redact_options: options });
    await r().confirm(1);
    expect(r().get(1).phase).toBe("confirm");

    const applied = { status: "applied", state, marks: 1, sanitize: {}, verify: { leaks: [] }, savedTo: "C:\\a_Redacted.pdf" };
    reply({ redact_apply: applied });
    await r().apply(1);
    // Only a flag goes over: the save target is picked on the Rust side.
    expect(inv).toHaveBeenCalledWith("redact_apply", { doc: 1, options: { sanitize: options.sanitize, save: true } });
    expect(JSON.stringify(inv.mock.calls[inv.mock.calls.length - 1])).not.toMatch(/Redacted\.pdf/);
    expect(r().get(1).phase).toBe("report");
    expect(r().get(1).marks).toEqual([]);
    expect(applyState).toHaveBeenCalledWith(1, state);

    r().done(1);
    expect(r().get(1).phase).toBe("marking");
  });

  it("a refusal keeps the marks and says what leaked; a cancelled dialog goes back to the confirm", async () => {
    useRedact.setState({ docs: { 1: { ...r().get(1), marks: [mark(0, 9)], options, phase: "confirm" } } as never });
    reply({ redact_apply: { status: "refused", leaks: [{ needle: "x", place: "page 1", critical: true }] } });
    await r().apply(1);
    expect(r().get(1).phase).toBe("refused");
    expect(r().get(1).marks).toHaveLength(1);
    expect(applyState).not.toHaveBeenCalled();

    useRedact.setState({ docs: { 1: { ...r().get(1), phase: "confirm" } } });
    reply({ redact_apply: { status: "cancelled" } });
    await r().apply(1);
    expect(r().get(1).phase).toBe("confirm");
  });

  it("an engine error goes back to the confirm and is shown", async () => {
    useRedact.setState({ docs: { 1: { ...r().get(1), marks: [mark(0, 9)], options, phase: "confirm" } } as never });
    reply({});
    await r().apply(1);
    expect(r().get(1).phase).toBe("confirm");
    expect(r().get(1).error).toMatch(/redact_apply/);
  });
});

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { applyState } from "./edit";
import { markLabel, SANITIZE_OFF, useRedact, type Hit, type Mark } from "./redact";

const inv = vi.mocked(invoke);
const hit = (page: number, text: string): Hit => ({ page, text, quads: [[0, 0, 1, 0, 0, 1, 1, 1]], before: "", after: "" });
const mark = (page: number, xref: number): Mark => ({ page, xref, quads: [], wholePage: false, label: null });
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

  it("counts every pattern when asked, and remembers the query of the hits", async () => {
    reply({ redact_find: [hit(0, "a"), hit(1, "b")] });
    await r().loadCounts(1);
    expect(inv).toHaveBeenCalledTimes(6);
    expect(r().get(1).counts).toEqual({ personnummer: 2, ssn: 2, card: 2, iban: 2, email: 2, phone: 2 });
    await r().find(1, { kind: "pattern", pattern: "iban" });
    expect(r().get(1).query).toEqual({ kind: "pattern", pattern: "iban" });
    r().clearFind(1);
    expect(r().get(1)).toMatchObject({ query: null, hits: [], checked: [] });
  });

  it("\"Also remove hidden information\" off sends all options false, on asks for the defaults", async () => {
    reply({ redact_options: options });
    await r().removeHidden(1, false);
    expect(inv).toHaveBeenLastCalledWith("redact_options", { doc: 1, sanitize: SANITIZE_OFF });
    expect(Object.values(SANITIZE_OFF).some(Boolean)).toBe(false);
    await r().removeHidden(1, true);
    expect(inv).toHaveBeenLastCalledWith("redact_options", { doc: 1, sanitize: undefined });
  });

  it("labels a mark with the engine's text, or says area or page", async () => {
    const q: Mark["quads"] = [[10, 10, 50, 10, 10, 20, 50, 20]];
    const h = { page: 1, text: "alex@example.com", quads: q, before: "Mail ", after: " now" };
    reply({ redact_find: [h] });
    await r().find(1, { kind: "pattern", pattern: "email" });
    reply({ redact_mark: state, redact_list_marks: [{ page: 1, xref: 7, quads: q, wholePage: false, label: h.text }] });
    await r().markChecked(1);
    expect(markLabel(r().get(1).marks[0])).toBe("alex@example.com");

    const area = { page: 3, xref: 8, quads: q, wholePage: false, label: null };
    expect(markLabel(area)).toBe("Area on page 4");
    expect(markLabel({ ...area, page: 8, xref: 9, wholePage: true })).toBe("Page 9");
  });
});

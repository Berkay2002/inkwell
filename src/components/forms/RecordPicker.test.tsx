import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../../store", () => ({ useStore: { getState: () => ({ setUi: vi.fn(), docs: {} }) } }));
vi.mock("../../lib/api", () => ({ pickSave: vi.fn() }));
vi.mock("../../lib/edit", () => ({ applyState: vi.fn() }));
vi.mock("../../lib/annot", () => ({ useAnnot: { getState: () => ({ refresh: vi.fn() }) } }));
vi.mock("../../lib/forms", () => ({ useForms: { getState: () => ({ refresh: vi.fn() }) } }));

import { useExchange } from "../../lib/dataExchange";
import { Picker, RecordPicker, rowText } from "./RecordPicker";

beforeEach(() => useExchange.setState({ picker: null }));

describe("RecordPicker", () => {
  it("shows nothing until a file with several records is imported", () => {
    expect(renderToStaticMarkup(<RecordPicker />)).toBe("");
  });

  it("lists each record by its label, numbered as a spreadsheet numbers its rows", () => {
    const html = renderToStaticMarkup(<Picker name="people.csv" labels={["Asa Berg", "Ole Dahl"]} />);
    expect(html).toContain('role="dialog"');
    expect(html).toContain("people.csv has 2 records");
    expect(html).toContain(rowText(0, "Asa Berg"));
    expect(rowText(0, "Asa Berg")).toBe("2. Asa Berg");
    expect(html).toContain("3. Ole Dahl");
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain("Import record");
  });

  it("a label with markup is text, never HTML", () => {
    const html = renderToStaticMarkup(<Picker name="a.csv" labels={["<img src=x onerror=alert(1)>"]} />);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });

  it("Cancel and the answer go through the store", () => {
    const answer = vi.fn();
    useExchange.setState({ picker: { name: "a.csv", labels: ["x", "y"] }, chooseRecord: answer });
    useExchange.getState().chooseRecord(1);
    expect(answer).toHaveBeenCalledWith(1);
  });
});

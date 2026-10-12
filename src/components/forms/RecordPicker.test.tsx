import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactElement, ReactNode } from "react";
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
    const html = renderToStaticMarkup(<Picker name="people.csv" labels={["Asa Berg", "Ole Dahl"]} rows={[2, 5]} />);
    expect(html).toContain('role="dialog"');
    expect(html).toContain("people.csv has 2 records");
    expect(html).toContain(rowText(2, "Asa Berg"));
    expect(rowText(2, "Asa Berg")).toBe("2. Asa Berg");
    // the row after a skipped blank line is 5, not 3
    expect(html).toContain("5. Ole Dahl");
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain("Import record");
  });

  it("a label with markup is text, never HTML", () => {
    const html = renderToStaticMarkup(<Picker name="a.csv" labels={["<img src=x onerror=alert(1)>"]} rows={[2]} />);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });

  it("the buttons and keys call the store's answer: Import record, Cancel, Enter on a row, Escape", () => {
    const choose = vi.fn();
    useExchange.setState({ chooseRecord: choose });
    // the element tree, got by rendering Picker inside a component (it has a hook), so each `props` is its own
    let tree: ReactElement | undefined;
    const Probe = () => (tree = Picker({ name: "a.csv", labels: ["x", "y"], rows: [2, 3] }) as ReactElement);
    renderToStaticMarkup(<Probe />);
    type P = { children?: ReactNode; className?: string; onClick?: () => void; onKeyDown?: (e: unknown) => void };
    const all: ReactElement<P>[] = [];
    const walk = (n: ReactNode) => {
      if (Array.isArray(n)) return n.forEach(walk);
      if (!n || typeof n !== "object" || !("props" in n)) return;
      const el = n as ReactElement<P>;
      all.push(el);
      walk(el.props.children);
    };
    walk(tree);
    const text = (n: ReactNode): string => (Array.isArray(n) ? n.map(text).join("") : typeof n === "string" ? n : "");
    const button = (label: string) => all.find((e) => e.type === "button" && text(e.props.children) === label)!;
    const key = (k: string) => ({ key: k, preventDefault: vi.fn(), stopPropagation: vi.fn() });

    button("Import record").props.onClick!();
    expect(choose).toHaveBeenLastCalledWith(0);
    button("Cancel").props.onClick!();
    expect(choose).toHaveBeenLastCalledWith(null);
    button("3. y").props.onKeyDown!(key("Enter"));
    expect(choose).toHaveBeenLastCalledWith(1);
    all.find((e) => e.props.className?.includes("fm-records"))!.props.onKeyDown!(key("Escape"));
    expect(choose).toHaveBeenLastCalledWith(null);
  });
});

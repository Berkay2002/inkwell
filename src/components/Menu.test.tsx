import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { ReactElement } from "react";
import { type MenuItem, MenuRow } from "./Menu";

/** The row's button element, got by rendering it inside a component (a hook needs a render), so `props.onClick` is its own. */
function rowOf(it: MenuItem, onDone: () => void) {
  let el: ReactElement<{ onClick: () => void }> | undefined;
  const Probe = () => (el = MenuRow({ it, onDone }) as ReactElement<{ onClick: () => void }>);
  renderToStaticMarkup(<Probe />);
  return el!;
}

const off = { label: "Import comments…", disabled: true, title: "Certified", onSelect: vi.fn() };

describe("MenuRow", () => {
  it("an off item stays focusable: aria-disabled and its title, never the disabled attribute", () => {
    const html = renderToStaticMarkup(<MenuRow it={off} onDone={() => {}} />);
    expect(html).toContain('aria-disabled="true"');
    expect(html).toContain('title="Certified"');
    expect(html).not.toMatch(/\sdisabled(=|\s|>)/);
  });

  it("an off item's reason is in the DOM and aria-describedby points at it", () => {
    const html = renderToStaticMarkup(<MenuRow it={off} onDone={() => {}} />);
    const id = /aria-describedby="([^"]+)"/.exec(html)?.[1];
    expect(id).toBeTruthy();
    expect(html).toContain(`id="${id}"`);
    expect(html).toMatch(new RegExp(`<span id="${id}"[^>]*>Certified</span>`));
  });

  it("an item that is on, or off without a reason, describes nothing", () => {
    for (const it of [{ label: "Export", title: "Saves a copy", onSelect: () => {} }, { label: "Import", disabled: true, onSelect: () => {} }]) {
      expect(renderToStaticMarkup(<MenuRow it={it} onDone={() => {}} />)).not.toContain("aria-describedby");
    }
  });

  it("an on item has no aria-disabled", () => {
    expect(renderToStaticMarkup(<MenuRow it={{ label: "Export", onSelect: () => {} }} onDone={() => {}} />)).not.toContain("aria-disabled");
  });

  it("activating an off item neither closes the menu nor runs it; an on item does both", () => {
    const done = vi.fn();
    const click = (it: MenuItem) => rowOf(it, done).props.onClick();
    click(off);
    expect(done).not.toHaveBeenCalled();
    expect(off.onSelect).not.toHaveBeenCalled();
    const onSelect = vi.fn();
    click({ label: "Export", onSelect });
    expect(done).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});

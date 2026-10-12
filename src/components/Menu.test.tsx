import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { ReactElement } from "react";
import { MenuRow } from "./Menu";

const off = { label: "Import comments…", disabled: true, title: "Certified", onSelect: vi.fn() };

describe("MenuRow", () => {
  it("an off item stays focusable: aria-disabled and its title, never the disabled attribute", () => {
    const html = renderToStaticMarkup(<MenuRow it={off} onDone={() => {}} />);
    expect(html).toContain('aria-disabled="true"');
    expect(html).toContain('title="Certified"');
    expect(html).not.toMatch(/\sdisabled(=|\s|>)/);
  });

  it("an on item has no aria-disabled", () => {
    expect(renderToStaticMarkup(<MenuRow it={{ label: "Export", onSelect: () => {} }} onDone={() => {}} />)).not.toContain("aria-disabled");
  });

  it("activating an off item neither closes the menu nor runs it; an on item does both", () => {
    const done = vi.fn();
    const click = (it: Parameters<typeof MenuRow>[0]["it"]) => (MenuRow({ it, onDone: done }) as ReactElement<{ onClick: () => void }>).props.onClick();
    click(off);
    expect(done).not.toHaveBeenCalled();
    expect(off.onSelect).not.toHaveBeenCalled();
    const onSelect = vi.fn();
    click({ label: "Export", onSelect });
    expect(done).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});

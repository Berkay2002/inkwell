import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ErrorBoundary } from "./ErrorBoundary";

/** An instance with its state wired the way React does (setState merges), for calling render() without a DOM. */
function boundary(props: { name: string; variant?: "dock" | "fill" | "overlay" }) {
  const b = new ErrorBoundary({ ...props, children: "child" });
  b.setState = ((s: object) => Object.assign(b.state, typeof s === "function" ? (s as (p: unknown) => object)(b.state) : s)) as typeof b.setState;
  return b;
}

describe("ErrorBoundary", () => {
  it("renders its children until something throws", () => {
    expect(boundary({ name: "Search" }).render()).toBe("child");
  });

  it("a throw becomes a card with the panel's name, the message and Try again / Close", () => {
    const b = boundary({ name: "Search", variant: "dock" });
    Object.assign(b.state, ErrorBoundary.getDerivedStateFromError(new Error("Rendered more hooks than during the previous render.")));
    const html = renderToStaticMarkup(b.render() as never);
    expect(html).toContain('role="alert"');
    expect(html).toContain("Search hit a problem");
    expect(html).toContain("Rendered more hooks");
    expect(html).toContain("err-boundary dock");
    expect(html).toContain("Try again");
    expect(html).toContain("Close");
  });

  it("a thrown string still gets a message", () => {
    expect(ErrorBoundary.getDerivedStateFromError("boom").error?.message).toBe("boom");
  });

  it("Try again renders the children again; Close renders nothing", () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const b = boundary({ name: "Search" });
    Object.assign(b.state, ErrorBoundary.getDerivedStateFromError(new Error("x")));
    type Card = { props: { onRetry: () => void; onClose: () => void } };
    const card = b.render() as unknown as Card;
    card.props.onClose();
    expect(b.render()).toBeNull();
    Object.assign(b.state, ErrorBoundary.getDerivedStateFromError(new Error("y")));
    (b.render() as unknown as Card).props.onRetry();
    expect(b.render()).toBe("child");
    log.mockRestore();
  });

  it("the card goes away when the reset key changes", () => {
    const b = boundary({ name: "Search" });
    Object.assign(b.state, ErrorBoundary.getDerivedStateFromError(new Error("x")));
    const next = new ErrorBoundary({ name: "Search", children: "child", resetKey: 2 });
    Object.assign(b, { props: next.props });
    b.componentDidUpdate({ name: "Search", children: "child", resetKey: 1 });
    expect(b.state.error).toBeNull();
  });
});

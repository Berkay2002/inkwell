import { Component, type ReactNode } from "react";
import "./errorBoundary.css";

/** dock: a right-rail panel; fill: a region that fills its parent; overlay: a fixed card over the window (sheets, strips, dialogs). */
export type Variant = "dock" | "fill" | "overlay";

type Props = {
  /** What crashed, shown in the card ("Search", "The page view"). */
  name: string;
  variant?: Variant;
  /** The card goes away when this changes (another document became active). */
  resetKey?: unknown;
  children: ReactNode;
};
type State = { error: Error | null; closed: boolean };

/** One panel's crash shows a recoverable card in its place; the rest of the window keeps working. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, closed: false };

  static getDerivedStateFromError(error: unknown): Partial<State> {
    return { error: error instanceof Error ? error : new Error(String(error)), closed: false };
  }

  componentDidCatch(error: Error, info: { componentStack?: string | null }) {
    console.error(`${this.props.name} crashed:`, error, info.componentStack);
  }

  componentDidUpdate(prev: Props) {
    if (this.state.error && prev.resetKey !== this.props.resetKey) this.setState({ error: null, closed: false });
  }

  render() {
    const { error, closed } = this.state;
    if (!error) return this.props.children;
    if (closed) return null;
    return (
      <ErrorCard
        name={this.props.name}
        variant={this.props.variant ?? "fill"}
        message={error.message}
        onRetry={() => this.setState({ error: null })}
        onClose={() => this.setState({ closed: true })}
      />
    );
  }
}

export function ErrorCard({ name, variant, message, onRetry, onClose }: { name: string; variant: Variant; message: string; onRetry: () => void; onClose: () => void }) {
  return (
    <div className={`err-boundary ${variant}`} role="alert">
      <div className="err-card">
        <h2>{name} hit a problem</h2>
        <p>The rest of the app is still working. Your document is unchanged.</p>
        <pre className="err-msg">{message}</pre>
        <div className="err-actions">
          <button className="err-retry" onClick={onRetry}>
            Try again
          </button>
          <button className="err-close" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

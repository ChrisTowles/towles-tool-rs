import { Component, Fragment, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

/**
 * Isolates a render crash to one subtree. Each screen's mount point is wrapped
 * so one screen throwing on bad live-snapshot data shows an inline card instead
 * of white-screening the whole app — and, critically, leaves the CloseGuard
 * dialog and the window's close path mounted, so the window stays closable
 * (the Rust side intercepts close while live shells exist and needs the React
 * tree alive to resolve it). Reset re-mounts the wrapped children.
 */
type Props = { children: ReactNode; label?: string };
type State = { error: Error | null; resetKey: number };

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, resetKey: 0 };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Screen crashed:", error, info.componentStack);
  }

  private reset = () => {
    this.setState((s) => ({ error: null, resetKey: s.resetKey + 1 }));
  };

  render() {
    const { error, resetKey } = this.state;
    if (error) {
      const label = this.props.label ? ` — ${this.props.label}` : "";
      return (
        <div className="flex h-full items-center justify-center p-6">
          <div className="w-full max-w-md rounded-lg border bg-card p-4 text-card-foreground">
            <div className="text-sm font-medium text-destructive">
              This screen crashed{label}
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              The rest of the app is still running. Reset to reopen it.
            </p>
            <pre className="mt-2 max-h-32 overflow-auto rounded-md bg-muted p-2 font-mono text-xs text-muted-foreground">
              {error.message}
            </pre>
            <Button size="sm" className="mt-3" onClick={this.reset}>
              Reset
            </Button>
          </div>
        </div>
      );
    }
    // Keying the children remounts them fresh on reset, clearing any state that
    // led to the crash.
    return <Fragment key={resetKey}>{this.props.children}</Fragment>;
  }
}

import { Component, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

type Props = { onSwitchToTable: () => void; children: ReactNode };
type State = { hasError: boolean };

/** Sigma renders synchronously into a foreign-owned canvas; a bug there (or a browser
 *  quirk it hits) must not take the whole app down with it (no error boundary otherwise
 *  catches it, and React unmounts the entire tree on an uncaught render error). This
 *  boundary is the map's blast door: on error, the rest of the app — the tree view, the
 *  detail panel — keeps working, and the user gets a way back to it. */
export class MapErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false };

  static getDerivedStateFromError(): State {
    return { hasError: true };
  }

  render() {
    if (this.state.hasError) {
      return (
        <div role="alert" className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-sm">
          <p>The map could not be drawn.</p>
          <Button type="button" variant="outline" onClick={this.props.onSwitchToTable}>Switch to table</Button>
        </div>
      );
    }
    return this.props.children;
  }
}

import { recordClientMetric } from "../api/clientMetrics.ts";
import { metric } from "../api/clientMetricSchema.ts";
import { Component, type ComponentChildren } from "preact";

export class AppErrorBoundary extends Component<
  { children: ComponentChildren },
  { failed: boolean }
> {
  override state = { failed: false };
  static override getDerivedStateFromError() {
    return { failed: true };
  }
  override componentDidCatch(error: unknown) {
    recordClientMetric(metric.renderError);
    console.error("Application rendering failed", error);
  }
  render() {
    return this.state.failed ? (
      <div class="loading" role="alert">
        <p>Unable to display Commonroom. Reload to retry.</p>
      </div>
    ) : (
      this.props.children
    );
  }
}

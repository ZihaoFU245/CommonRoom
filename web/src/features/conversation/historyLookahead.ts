/** Predict a bounded paging margin from scroll velocity and observed HTTP RTT. */
export class HistoryLookahead {
  private latency = 500;
  private velocity = 0;
  private previous: { top: number; time: number } | undefined;
  sample(milliseconds: number) {
    if (Number.isFinite(milliseconds) && milliseconds >= 0)
      this.latency = this.latency * 0.7 + Math.min(milliseconds, 15000) * 0.3;
  }
  reset() {
    this.previous = undefined;
    this.velocity = 0;
  }
  observe(top: number, height: number, time: number) {
    const previous = this.previous;
    this.previous = { top, time };
    const movement = previous ? top - previous.top : 0;
    if (previous && time > previous.time) {
      const speed = Math.min(4, Math.abs(movement) / (time - previous.time));
      this.velocity =
        time - previous.time > 500 ? speed : this.velocity * 0.7 + speed * 0.3;
    }
    const viewport = Math.max(240, height);
    return {
      direction: Math.sign(movement),
      distance: Math.min(
        viewport * 3,
        Math.max(viewport, this.velocity * (this.latency + 150)),
      ),
    };
  }
}

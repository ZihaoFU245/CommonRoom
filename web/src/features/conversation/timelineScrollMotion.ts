/** Scroll to a virtual row whose measured position can change during motion. */
export class TimelineScrollMotion {
  private frame = 0;
  private target: (() => number | undefined) | null = null;
  private viewport: HTMLElement;
  private update: () => void;

  constructor(viewport: HTMLElement, update: () => void) {
    this.viewport = viewport;
    this.update = update;
  }

  get active(): boolean {
    return this.target !== null;
  }

  cancel(): void {
    cancelAnimationFrame(this.frame);
    this.target = null;
  }

  finish(): void {
    const top = this.target?.();
    this.cancel();
    if (top !== undefined) this.viewport.scrollTop = top;
    this.update();
  }

  /** Retain an instant restore while the new virtual window is being measured. */
  snapTo(target: () => number | undefined): void {
    this.cancel();
    this.target = target;
    let previous: number | undefined;
    let stable = 0;
    const started = performance.now();
    const settle = (now: number) => {
      const top = this.target?.();
      if (top === undefined) {
        this.cancel();
        return;
      }
      stable =
        previous !== undefined && Math.abs(top - previous) < 0.5
          ? stable + 1
          : 0;
      previous = top;
      this.viewport.scrollTop = top;
      this.update();
      if (stable >= 2 || now - started >= 300) this.cancel();
      else this.frame = requestAnimationFrame(settle);
    };
    settle(started);
  }

  scrollTo(target: () => number | undefined, complete?: () => void): void {
    this.cancel();
    this.target = target;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      this.finish();
      complete?.();
      return;
    }
    const from = this.viewport.scrollTop;
    const started = performance.now();
    const tick = (now: number) => {
      const top = this.target?.();
      if (top === undefined) {
        this.cancel();
        return;
      }
      const progress = Math.min(1, Math.max(0, (now - started) / 300));
      this.viewport.scrollTop = from + (top - from) * (1 - (1 - progress) ** 3);
      this.update();
      if (progress < 1) this.frame = requestAnimationFrame(tick);
      else {
        this.cancel();
        complete?.();
      }
    };
    this.frame = requestAnimationFrame(tick);
  }
}

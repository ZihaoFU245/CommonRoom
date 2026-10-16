// This tiny standalone bootstrap runs only in an opaque-origin sandbox.
function observeSize(): void {
  let last = 0;
  let scheduled = false;
  const report = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      const height = Math.max(
        128,
        Math.min(800, document.body?.scrollHeight ?? 360),
      );
      if (height !== last) {
        last = height;
        parent.postMessage({ kind: "markdown-height", height }, "*");
      }
    });
  };
  addEventListener(
    "DOMContentLoaded",
    () => {
      new ResizeObserver(report).observe(document.documentElement);
      report();
    },
    { once: true },
  );
  addEventListener("load", report, { once: true });
}
function receive(event: MessageEvent<unknown>): void {
  if (
    event.source !== parent ||
    !event.data ||
    typeof event.data !== "object" ||
    !("kind" in event.data) ||
    event.data.kind !== "markdown-render" ||
    !("source" in event.data) ||
    typeof event.data.source !== "string"
  )
    return;
  removeEventListener("message", receive);
  const policy =
    document
      .querySelector('meta[http-equiv="Content-Security-Policy"]')
      ?.getAttribute("content") ?? "default-src 'none'";
  const prefix =
    '<!doctype html><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="' +
    policy.replaceAll("&", "&amp;").replaceAll('"', "&quot;") +
    '"><script>(' +
    observeSize.toString() +
    ")();<\/script>";
  document.open();
  document.write(prefix + event.data.source);
  document.close();
}
addEventListener("message", receive);

import { requestAbort } from "../../api/requestAbort.ts";

let script: URL | undefined;
let pending: Promise<void> | undefined;
let confirmed: ServiceWorker | null = null;

/** Start once per document; managed files never take an uncached loading path. */
export function startMediaCache(url: URL): void {
  script = url;
  ensureMediaCache().catch(() => {
    // File controls surface initialization errors when media is requested.
  });
}

/** Readiness requires an acknowledged handshake with the current controller. */
export function mediaCacheReady(): boolean {
  return (
    confirmed !== null && navigator.serviceWorker?.controller === confirmed
  );
}

export function ensureMediaCache(): Promise<void> {
  if (mediaCacheReady()) return Promise.resolve();
  pending ??= connect().finally(() => {
    pending = undefined;
  });
  return pending;
}

async function connect(): Promise<void> {
  if (!script || typeof navigator === "undefined" || !navigator.serviceWorker)
    throw new Error("Open this app over HTTPS or localhost to load media.");
  const service = navigator.serviceWorker;
  const cancellation = requestAbort(15000);
  let registration: ServiceWorkerRegistration | undefined;
  let channel: MessageChannel | undefined;
  let finish = () => {};
  let fail: (error: unknown) => void = () => {};
  const ready = new Promise<void>((resolve, reject) => {
    finish = resolve;
    fail = reject;
  });
  function claim() {
    const worker = registration?.active ?? service.controller;
    if (!worker) return;
    channel?.port1.close();
    channel = new MessageChannel();
    channel.port1.onmessage = (event: MessageEvent<unknown>) => {
      if (
        event.data === "commonroom-media:ready" &&
        service.controller === worker
      ) {
        confirmed = worker;
        finish();
      }
    };
    worker.postMessage("commonroom-media:claim", [channel.port2]);
  }
  const abort = () =>
    fail(new Error("Media cache could not start. Reload or retry."));
  service.addEventListener("controllerchange", claim);
  cancellation.signal.addEventListener("abort", abort, { once: true });
  try {
    // Register without blocking the deadline on a stalled worker download.
    service
      .register(script, {
        type: "module",
        scope: new URL("./", document.baseURI).pathname,
        updateViaCache: "none",
      })
      .then((value) => {
        if (cancellation.signal.aborted) return;
        registration = value;
        claim();
      }, fail)
      .catch(fail);
    await ready;
  } finally {
    cancellation.dispose();
    service.removeEventListener("controllerchange", claim);
    channel?.port1.close();
  }
}

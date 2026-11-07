import { mediaFileUrl } from "../../api/mediaFile.ts";
import { httpImageCache } from "./httpImageCache.ts";
import { isResolution, MediaAccess, MediaAccessError } from "./mediaAccess.ts";
import type { Resolution } from "./mediaAccess.ts";
import { requestAbort } from "../../api/requestAbort.ts";

let script: URL | undefined;
let pending: Promise<void> | undefined;
let confirmed: ServiceWorker | null = null;

/** Service workers are unavailable on non-loopback HTTP, including Tailscale. */
export function directMediaRequired(): boolean {
  return (
    new URL(document.baseURI).protocol === "http:" &&
    (typeof navigator === "undefined" || !navigator.serviceWorker)
  );
}

/** Start once per document; HTTP without workers uses authorized signed links. */
export function startMediaCache(url: URL): void {
  script = url;
  if (directMediaRequired()) return;
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
      })
      .catch(fail);
    await ready;
  } finally {
    cancellation.dispose();
    service.removeEventListener("controllerchange", claim);
    channel?.port1.close();
  }
}

const metadataAccess = new MediaAccess();
const reads = new Set<{ id: string; invalidated: boolean }>();
let clearing: Promise<void> = Promise.resolve();
interface Watcher {
  count: number;
  revision: number;
  due: number;
  run: (refresh?: boolean) => void;
}
const watchers = new Map<string, Watcher>();
let heartbeat: ReturnType<typeof setTimeout> | undefined;
function scheduleHeartbeat(): void {
  clearTimeout(heartbeat);
  if (document.hidden || !watchers.size) return;
  const due = Math.min(
    ...Array.from(watchers.values(), (watcher) => watcher.due),
  );
  if (!Number.isFinite(due)) return;
  heartbeat = setTimeout(
    () => {
      const now = Date.now();
      for (const watcher of watchers.values())
        if (watcher.due <= now) watcher.run(true);
      scheduleHeartbeat();
    },
    Math.max(1000, due - Date.now()),
  );
}
function resumeWatches(): void {
  for (const watcher of watchers.values()) watcher.run();
  scheduleHeartbeat();
}

async function workerRequest(
  data: unknown,
  selected?: ServiceWorker,
): Promise<unknown> {
  if (!selected) await ensureMediaCache();
  const worker = selected ?? navigator.serviceWorker.controller;
  if (!worker) throw new Error("Media worker unavailable.");
  const channel = new MessageChannel();
  const cancellation = requestAbort(15000);
  let abort = () => {};
  try {
    return await new Promise<unknown>((resolve, reject) => {
      abort = () =>
        reject(new Error("Media worker did not respond. Retry or reload."));
      cancellation.signal.addEventListener("abort", abort, { once: true });
      channel.port1.onmessage = (event: MessageEvent<unknown>) =>
        resolve(event.data);
      worker.postMessage(data, [channel.port2]);
    });
  } finally {
    cancellation.dispose();
    cancellation.signal.removeEventListener("abort", abort);
    channel.port1.close();
  }
}

/** Clear read authority while retaining disposable, private cached bytes. */
export function invalidateMediaReads(ids?: readonly string[]): Promise<void> {
  if (ids?.length === 0) return clearing;
  for (const read of reads)
    if (!ids || ids.includes(read.id)) read.invalidated = true;
  metadataAccess.clear(ids);
  httpImageCache.invalidate(ids);
  clearing = clearing
    .catch(() => {})
    .then(async () => {
      if (script && !directMediaRequired()) {
        await ensureMediaCache();
        await workerRequest({ kind: "commonroom-media:clear", ids });
      }
    })
    .catch((error: unknown) => {
      console.error("Media cache invalidation failed", error);
    });
  for (const [href, watcher] of watchers) {
    if (ids && !ids.includes(mediaFileUrl(href)?.id ?? "")) continue;
    watcher.run();
  }
  return clearing;
}

export async function resolveMediaRead(
  href: string,
  refresh = false,
  download = false,
  link = false,
): Promise<Resolution> {
  for (let attempt = 0; ; attempt++) {
    // Select once: a controller transition cannot change this operation's backend.
    if (link || mediaCacheReady()) await clearing;
    const direct = directMediaRequired();
    const worker = mediaCacheReady() ? confirmed : null;
    const read = { id: mediaFileUrl(href)?.id ?? "", invalidated: false };
    reads.add(read);
    try {
      const packet: unknown = direct
        ? { value: await httpImageCache.resolve(href, refresh, download, link) }
        : !link && !worker
          ? { value: await metadataAccess.load(href, false, refresh, download) }
          : await workerRequest(
              {
                kind: "commonroom-media:resolve",
                href,
                refresh,
                download,
                link,
              },
              worker ?? undefined,
            );
      if (read.invalidated)
        throw new DOMException("Media access changed", "AbortError");
      if (!packet || typeof packet !== "object")
        throw new Error("Invalid media worker response.");
      if ("value" in packet && isResolution(packet.value)) return packet.value;
      throw new MediaAccessError(
        "status" in packet && typeof packet.status === "number"
          ? packet.status
          : 503,
        "error" in packet && typeof packet.error === "string"
          ? packet.error
          : "Media unavailable.",
      );
    } catch (error) {
      if (attempt === 0 && read.invalidated) continue;
      throw error;
    } finally {
      reads.delete(read);
    }
  }
}

/** One heartbeat renews due mounted media together through the batch resolver. */
export function watchMediaRead(href: string): () => void {
  const existing = watchers.get(href);
  if (existing) {
    existing.count++;
    return () => releaseWatch(href);
  }
  const watcher: Watcher = {
    count: 1,
    revision: 0,
    due: Infinity,
    run: () => {},
  };
  watcher.run = (refresh = false) => {
    const revision = ++watcher.revision;
    watcher.due = Infinity;
    if (watchers.get(href) !== watcher || document.hidden) return;
    resolveMediaRead(href, refresh)
      .then((value) => {
        if (watchers.get(href) !== watcher || watcher.revision !== revision)
          return;
        if (value.info.expired || value.info.unavailable) return;
        // Round long leases down to a shared minute, retaining expiry headroom.
        watcher.due =
          value.renewAt - Date.now() > 60000
            ? Math.floor(value.renewAt / 60000) * 60000
            : value.renewAt;
      })
      .catch((error: unknown) => {
        if (watchers.get(href) !== watcher || watcher.revision !== revision)
          return;
        if (!(error instanceof DOMException && error.name === "AbortError"))
          console.error("Media lease renewal failed", error);
        watcher.due = Date.now() + 10000;
      })
      .finally(scheduleHeartbeat);
  };
  if (!watchers.size)
    document.addEventListener("visibilitychange", resumeWatches);
  watchers.set(href, watcher);
  watcher.run();
  return () => releaseWatch(href);
}
function releaseWatch(href: string): void {
  const watcher = watchers.get(href);
  if (!watcher || --watcher.count > 0) return;
  watchers.delete(href);
  if (!watchers.size)
    document.removeEventListener("visibilitychange", resumeWatches);
  scheduleHeartbeat();
}

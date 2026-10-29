/** Persistent immutable bytes. Every reuse first checks live server authorization. */
import { rangeStream } from "./rangeStream.ts";
const cacheName = "commonroom-media-v2";
const recencyCacheName = "commonroom-media-recency-v1";
const byteLimit = 256 * 1024 * 1024;
const entryLimit = 128;
const touchInterval = 60_000;
const fillLimit = 4;
// Bound cloned response bodies waiting for serialized Cache writes as well as disk use.
const fillByteLimit = 256 * 1024 * 1024;
interface Fill {
  done: Promise<void>;
  size: number;
  release: () => void;
}
interface Metadata {
  size: number;
  content_type: string;
  filename: string;
  expired?: boolean;
}
async function waitForWrite(
  work: Promise<void> | undefined,
  signal: AbortSignal,
) {
  signal.throwIfAborted();
  if (!work) return;
  await new Promise<void>((resolve, reject) => {
    const abort = () => {
      signal.removeEventListener("abort", abort);
      const reason: unknown = signal.reason;
      reject(
        reason instanceof Error
          ? reason
          : new DOMException("Request aborted", "AbortError"),
      );
    };
    signal.addEventListener("abort", abort, { once: true });
    work.then(
      () => {
        signal.removeEventListener("abort", abort);
        resolve();
      },
      (error: unknown) => {
        signal.removeEventListener("abort", abort);
        reject(
          error instanceof Error ? error : new Error("Cache write failed."),
        );
      },
    );
  });
}
function metadata(value: unknown): value is Metadata {
  return (
    typeof value === "object" &&
    value !== null &&
    "size" in value &&
    typeof value.size === "number" &&
    Number.isSafeInteger(value.size) &&
    value.size > 0 &&
    "content_type" in value &&
    typeof value.content_type === "string" &&
    "filename" in value &&
    typeof value.filename === "string" &&
    (!("expired" in value) || typeof value.expired === "boolean")
  );
}
export function mediaCachePath(request: Request, base: string): URL | null {
  const url = new URL(request.url);
  const root = new URL(base);
  const prefix = ["api/media/", "api/avatars/"]
    .map((path) => new URL(path, root).pathname)
    .find((path) => url.pathname.startsWith(path));
  return request.method === "GET" &&
    url.origin === root.origin &&
    prefix !== undefined &&
    /^[0-9a-f-]{36}\/file$/iu.test(url.pathname.slice(prefix.length))
    ? url
    : null;
}
function headers(info: Metadata, download: boolean): Headers {
  const inline =
    !download &&
    (/^image\/(png|jpeg|gif|webp|avif)$/u.test(info.content_type) ||
      /^(audio|video)\//u.test(info.content_type));
  return new Headers({
    "Content-Type": info.content_type,
    "Content-Disposition": inline
      ? "inline"
      : `attachment; filename="download"; filename*=UTF-8''${encodeURIComponent(info.filename).replace(/['()*]/gu, (c) => `%${c.charCodeAt(0).toString(16)}`)}`,
    "Cache-Control": "private, no-store",
    "X-Content-Type-Options": "nosniff",
    "Accept-Ranges": "bytes",
  });
}
export class MediaCache {
  private writes: Promise<void> = Promise.resolve();
  private readonly fills = new Map<string, Fill>();
  private fillingBytes = 0;
  private readonly touched = new Map<string, number>();
  private readonly persistedAt = new Map<string, number>();
  private readonly pending = new Map<string, Promise<void>>();
  private index: { sizes: Map<string, number>; bytes: number } | null = null;
  private readonly storage: CacheStorage;
  private readonly network: typeof fetch;
  constructor(
    storage: CacheStorage,
    network: typeof fetch = (...args) => fetch(...args),
  ) {
    this.storage = storage;
    this.network = network;
  }
  private reserve(key: string, size: number): Fill | undefined {
    if (
      this.fills.size >= fillLimit ||
      this.fillingBytes + size > fillByteLimit
    )
      return undefined;
    let finish = () => {};
    const done = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const fill: Fill = {
      done,
      size,
      release: () => {
        if (this.fills.get(key) !== fill) return;
        this.fills.delete(key);
        this.fillingBytes -= fill.size;
        finish();
      },
    };
    this.fills.set(key, fill);
    this.fillingBytes += size;
    return fill;
  }
  // Reconcile keys across overlapping worker lifetimes, reusing known sizes.
  // Listing keys is cheap compared with reading every cached response on insert.
  private async accounting(cache: Cache) {
    const sizes = new Map<string, number>();
    let bytes = 0;
    for (const key of await cache.keys()) {
      let size = this.index?.sizes.get(key.url);
      if (size === undefined) {
        const saved = await cache.match(key);
        if (!saved) continue;
        size = Number(saved.headers.get("Content-Length")) || 0;
      }
      sizes.set(key.url, size);
      bytes += size;
    }
    this.index = { sizes, bytes };
    return this.index;
  }
  private forget(key: string) {
    if (!this.index) return;
    this.index.bytes -= this.index.sizes.get(key) ?? 0;
    this.index.sizes.delete(key);
  }
  private remember(key: string, size: number) {
    if (!this.index) return;
    this.forget(key);
    this.index.sizes.set(key, size);
    this.index.bytes += size;
  }
  private touch(key: string, keepAlive: (work: Promise<void>) => void): void {
    const now = Date.now();
    const previous = this.persistedAt.get(key);
    this.touched.delete(key);
    this.touched.set(key, now);
    if (this.touched.size > entryLimit) {
      const oldest = this.touched.keys().next().value;
      if (oldest !== undefined) {
        this.touched.delete(oldest);
        this.persistedAt.delete(oldest);
      }
    }
    if (previous !== undefined && now - previous < touchInterval) return;
    this.persistedAt.set(key, now);
    // Persist only a tiny timestamp. Immutable attachment bytes are never
    // deleted/re-written merely because a player seeks or a row remounts.
    const work = this.writes
      .then(async () => {
        const recency = await this.storage.open(recencyCacheName);
        await recency.put(
          key,
          new Response(null, { headers: { "X-Accessed-At": String(now) } }),
        );
        // Browser eviction can remove bodies without their metadata. Bound this
        // cache independently, including when body storage never reaches pressure.
        const records = await recency.keys();
        for (const record of records.slice(
          0,
          Math.max(0, records.length - entryLimit),
        ))
          await recency.delete(record);
      })
      .catch(() => {
        if (this.persistedAt.get(key) === now) this.persistedAt.delete(key);
      });
    this.writes = work;
    keepAlive(work);
  }
  private async oldestKey(index: { sizes: Map<string, number> }) {
    let oldest = index.sizes.keys().next().value;
    try {
      const recency = await this.storage.open(recencyCacheName);
      let timestamp = Infinity;
      const ranks = new Map(
        [...this.touched.keys()].map((key, rank) => [key, rank]),
      );
      const keys = [...index.sizes.keys()].sort(
        (a, b) => (ranks.get(a) ?? -1) - (ranks.get(b) ?? -1),
      );
      for (const key of keys) {
        const saved = await recency.match(key);
        const accessed = Math.max(
          this.touched.get(key) ?? 0,
          Number(saved?.headers.get("X-Accessed-At")) || 0,
        );
        if (accessed < timestamp) {
          timestamp = accessed;
          oldest = key;
        }
      }
      if (oldest !== undefined) await recency.delete(oldest);
      // Browser eviction and overlapping workers may leave orphaned metadata.
      for (const key of await recency.keys())
        if (!index.sizes.has(key.url)) await recency.delete(key);
    } catch {
      /* Storage failure falls back to insertion order. */
    }
    return oldest;
  }
  async respond(
    request: Request,
    url: URL,
    keepAlive: (work: Promise<void>) => void,
  ): Promise<Response> {
    // No offline fallback: logout, expiry, retraction and revoked access apply
    // equally to cached copies. The small info response never touches S3.
    const auth = await this.network(
      new URL(url.pathname.replace(/\/file$/u, "/info"), url),
      {
        credentials: "same-origin",
        cache: "no-store",
        signal: request.signal,
      },
    );
    if (!auth.ok) return new Response(null, { status: auth.status });
    const info: unknown = await auth.json();
    if (!metadata(info)) return Response.error();
    if (info.expired === true) return new Response(null, { status: 410 });
    const output = headers(info, url.searchParams.get("download") === "true");
    const range = request.headers.get("Range");
    const key = new URL(url.pathname, url);
    let cache: Cache | undefined;
    let reservation: Fill | undefined;
    try {
      cache = await this.storage.open(cacheName);
      await waitForWrite(this.pending.get(key.href), request.signal);
      let saved = await cache.match(key.href);
      if (!saved && !range) {
        // Reserve before the network await so simultaneous misses share the fill.
        // Every waiter has already performed its own live authorization check.
        const filling = this.fills.get(key.href);
        if (filling) {
          await waitForWrite(filling.done, request.signal);
          request.signal.throwIfAborted();
          saved = await cache.match(key.href);
        } else if (info.size <= byteLimit) {
          reservation = this.reserve(key.href, info.size);
        }
      }
      if (saved && !range) {
        this.touch(key.href, keepAlive);
        const size = saved.headers.get("Content-Length");
        if (size) output.set("Content-Length", size);
        const contentRange = saved.headers.get("Content-Range");
        if (contentRange) output.set("Content-Range", contentRange);
        return new Response(saved.body, {
          status: contentRange ? 206 : 200,
          headers: output,
        });
      }
      // A fully cached object can satisfy later native player byte ranges.
      if (range) {
        const full = saved;
        const match = /^bytes=(\d*)-(\d*)$/u.exec(range);
        if (full?.body && match && (match[1] || match[2])) {
          this.touch(key.href, keepAlive);
          const fullSize =
            Number(full.headers.get("Content-Length")) || info.size;
          const start = match[1]
            ? Number(match[1])
            : Math.max(0, fullSize - Number(match[2]));
          const end =
            match[1] && match[2]
              ? Math.min(Number(match[2]), fullSize - 1)
              : fullSize - 1;
          if (
            !Number.isSafeInteger(start) ||
            !Number.isSafeInteger(end) ||
            start > end ||
            start >= fullSize
          )
            return new Response(null, {
              status: 416,
              headers: { "Content-Range": `bytes */${fullSize}` },
            });
          output.set("Content-Length", String(end - start + 1));
          output.set("Content-Range", `bytes ${start}-${end}/${fullSize}`);
          return new Response(
            rangeStream(full.body, start, end, request.signal),
            {
              status: 206,
              headers: output,
            },
          );
        }
      }
    } catch {
      /* Storage-disabled/private browsers still load from the network. */
    }
    try {
      request.signal.throwIfAborted();
      const response = await this.network(url.href, {
        credentials: "same-origin",
        cache: "no-store",
        mode: "cors",
        headers: range ? { Range: range } : {},
        signal: request.signal,
      });
      if (!response.ok || (response.status !== 200 && response.status !== 206))
        return response;
      let contentRange = response.headers.get("Content-Range");
      // Content-Length is CORS-safelisted; older bucket CORS policies may hide
      // Content-Range. Reconstruct only an unambiguous single-byte-range response.
      if (response.status === 206 && !contentRange && range) {
        const match = /^bytes=(\d*)-(\d*)$/u.exec(range);
        const length = Number(response.headers.get("Content-Length"));
        if (match && (match[1] || match[2]) && length > 0) {
          const start = match[1]
            ? Number(match[1])
            : Math.max(0, info.size - Number(match[2]));
          const end = start + length - 1;
          if (Number.isSafeInteger(start) && start >= 0 && end < info.size)
            contentRange = `bytes ${start}-${end}/${info.size}`;
        }
      }
      if (response.status === 206 && !contentRange) return response;
      const size =
        Number(response.headers.get("Content-Length")) ||
        (response.status === 200 ? info.size : 0);
      if (Number.isSafeInteger(size) && size > 0)
        output.set("Content-Length", String(size));
      if (contentRange) output.set("Content-Range", contentRange);
      const result = new Response(response.body, {
        status: response.status,
        headers: output,
      });
      if (
        cache &&
        reservation &&
        !range &&
        response.status === 200 &&
        size > 0 &&
        this.fillingBytes + size - reservation.size <= fillByteLimit
      ) {
        const target = cache;
        const fill = reservation;
        // Cache only whole objects: seeking cannot fill the cache with overlapping
        // fragments. Serialized writes and recency touches share one order.
        const copy = new Response(result.clone().body, { headers: output });
        this.fillingBytes += size - fill.size;
        fill.size = size;
        reservation = undefined;
        const work = this.writes
          .then(async () => {
            const index = await this.accounting(target);
            // Concurrent misses may already have filled this key.
            await target.delete(key.href);
            this.forget(key.href);
            while (
              index.sizes.size >= entryLimit ||
              index.bytes + size > byteLimit
            ) {
              const oldest = await this.oldestKey(index);
              if (oldest === undefined) break;
              await target.delete(oldest);
              this.forget(oldest);
            }
            await target.put(key.href, copy);
            this.remember(key.href, size);
            this.touch(key.href, keepAlive);
          })
          .catch(() => {
            this.index = null;
            // Discard the tee branch if Cache rejects before reading its body.
            // Otherwise the continuing download can buffer into an unused copy.
            if (!copy.bodyUsed) copy.body?.cancel().catch(() => {});
            /* Quota/eviction failures must not break a download. */
          });
        this.writes = work;
        this.pending.set(key.href, work);
        keepAlive(
          work.finally(() => {
            fill.release();
            if (this.pending.get(key.href) === work)
              this.pending.delete(key.href);
          }),
        );
      }
      return result;
    } finally {
      reservation?.release();
    }
  }
}

/** Persistent immutable bytes. Every reuse first checks live server authorization. */
import { rangeStream } from "./rangeStream.ts";
import { requestAbort } from "../../api/requestAbort.ts";
import { isFileLink } from "../../api/mediaFile.ts";
const cacheName = "commonroom-media";
const recencyCacheName = "commonroom-media-recency";
const byteLimit = 256 * 1024 * 1024;
const entryLimit = 128;
const touchInterval = 60_000;
const fillLimit = 4;
// Bound cloned response bodies waiting for serialized Cache writes as well as disk use.
const fillByteLimit = 256 * 1024 * 1024;

// Cache maintenance is optional for a live response. Only browser storage and
// background cancellation failures may be ignored; programming failures escape
// through the worker event's lifetime promise.
function ignoreStorageFailure(error: unknown): void {
  if (
    error instanceof DOMException &&
    [
      "SecurityError",
      "QuotaExceededError",
      "UnknownError",
      "AbortError",
      "TimeoutError",
    ].includes(error.name)
  )
    return;
  throw error;
}

// Coordinate disk budgets across worker replacement and other same-origin tabs.
// Older/storage-limited browsers retain the instance's serialized write fallback.
function maintain(work: () => Promise<void>): Promise<void> {
  const locks = typeof navigator === "undefined" ? undefined : navigator.locks;
  return locks ? locks.request(cacheName, work) : work();
}
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
// Active consumers may need longer than two minutes on slow connections. Start
// the background-fill budget only when the original request is cancelled.
function fillCancellation(parent: AbortSignal) {
  const controller = new AbortController();
  let deadline: ReturnType<typeof requestAbort> | undefined;
  const expire = () => controller.abort(deadline?.signal.reason);
  const detach = () => {
    deadline = requestAbort(120_000);
    deadline.signal.addEventListener("abort", expire, { once: true });
  };
  if (parent.aborted) detach();
  else parent.addEventListener("abort", detach, { once: true });
  return {
    signal: controller.signal,
    dispose: () => {
      parent.removeEventListener("abort", detach);
      deadline?.signal.removeEventListener("abort", expire);
      deadline?.dispose();
    },
  };
}
async function waitForFill(work: Promise<void>, signal: AbortSignal) {
  signal.throwIfAborted();
  let abort = () => {};
  try {
    await Promise.race([
      work,
      new Promise<never>((_resolve, reject) => {
        abort = () => {
          const reason: unknown = signal.reason;
          reject(
            reason instanceof Error
              ? reason
              : new DOMException("Request aborted", "AbortError"),
          );
        };
        signal.addEventListener("abort", abort, { once: true });
      }),
    ]);
  } finally {
    signal.removeEventListener("abort", abort);
  }
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
// Cached bytes are disposable. Reject incompatible records without decoding or
// buffering their bodies; current authorization supplies presentation headers.
interface CachedFile {
  body: ReadableStream<Uint8Array>;
  size: number;
}
function cachedFile(response: Response): CachedFile | undefined {
  const length = response.headers.get("Content-Length") ?? "";
  const size = Number(length);
  const body = response.body;
  return response.status === 200 &&
    body !== null &&
    /^[1-9]\d*$/u.test(length) &&
    Number.isSafeInteger(size) &&
    size <= byteLimit &&
    !!response.headers.get("Content-Type") &&
    !response.headers.has("Content-Range") &&
    !response.headers.has("Content-Encoding")
    ? { body, size }
    : undefined;
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
/** Cache reads receive validated complete bytes, so range logic trusts their size. */
function cachedResponse(
  file: CachedFile,
  output: Headers,
  range: string | null,
  signal: AbortSignal,
): Response | undefined {
  if (!range) {
    output.set("Content-Length", String(file.size));
    return new Response(file.body, { headers: output });
  }
  const match = /^bytes=(\d*)-(\d*)$/u.exec(range);
  if (!match || (!match[1] && !match[2])) return undefined;
  const start = match[1]
    ? Number(match[1])
    : Math.max(0, file.size - Number(match[2]));
  const end =
    match[1] && match[2]
      ? Math.min(Number(match[2]), file.size - 1)
      : file.size - 1;
  if (
    !Number.isSafeInteger(start) ||
    !Number.isSafeInteger(end) ||
    start > end ||
    start >= file.size
  ) {
    file.body.cancel().catch(() => {});
    return new Response(null, {
      status: 416,
      headers: { "Content-Range": `bytes */${file.size}` },
    });
  }
  output.set("Content-Length", String(end - start + 1));
  output.set("Content-Range", `bytes ${start}-${end}/${file.size}`);
  return new Response(rangeStream(file.body, start, end, signal), {
    status: 206,
    headers: output,
  });
}

// Native Cache Storage on secure pages, or bounded tab-local storage on HTTP.
export type MediaCacheStore = Pick<Cache, "match" | "put" | "delete" | "keys">;
export interface MediaCacheStorage {
  open(name: string): Promise<MediaCacheStore>;
}

export class MediaCache {
  private writes: Promise<void> = Promise.resolve();
  private readonly fills = new Map<string, Fill>();
  private fillingBytes = 0;
  private readonly touched = new Map<string, number>();
  private readonly persistedAt = new Map<string, number>();
  private readonly recencyWrites = new Set<string>();
  private index: { sizes: Map<string, number>; bytes: number } | null = null;
  private readonly storage: MediaCacheStorage;
  private readonly network: typeof fetch;
  constructor(
    storage: MediaCacheStorage,
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
  private async read(cache: MediaCacheStore, key: string, info: Metadata) {
    const decode = (response: Response) =>
      response.headers.get("Content-Type") === info.content_type
        ? cachedFile(response)
        : undefined;
    let saved = await cache.match(key);
    let file = saved && decode(saved);
    if (!saved || file) return file;
    saved.body?.cancel().catch(() => {});
    // Recheck under the write lock: another worker may have replaced the record.
    const work = this.writes.then(() =>
      maintain(async () => {
        saved = await cache.match(key);
        file = saved && decode(saved);
        if (!saved || file) return;
        saved.body?.cancel().catch(() => {});
        await cache.delete(key);
        this.forget(key);
        await this.storage
          .open(recencyCacheName)
          .then((recency) => recency.delete(key))
          .catch(ignoreStorageFailure);
        saved = undefined;
      }),
    );
    this.writes = work.catch(() => {
      this.index = null;
    });
    await work;
    return file;
  }
  // Reconcile keys across overlapping worker lifetimes, reusing known sizes.
  // Listing keys is cheap compared with reading every cached response on insert.
  private async accounting(cache: MediaCacheStore) {
    const sizes = new Map<string, number>();
    let bytes = 0;
    for (const key of await cache.keys()) {
      let size = this.index?.sizes.get(key.url);
      if (size === undefined) {
        const saved = await cache.match(key);
        if (!saved) continue;
        size = cachedFile(saved)?.size;
        saved.body?.cancel().catch(() => {});
        if (size === undefined) {
          await cache.delete(key);
          continue;
        }
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
    if (
      (previous !== undefined && now - previous < touchInterval) ||
      this.recencyWrites.has(key) ||
      this.recencyWrites.size >= entryLimit
    )
      return;
    this.persistedAt.set(key, now);
    this.recencyWrites.add(key);
    // Persist only a tiny timestamp. Immutable attachment bytes are never
    // deleted/re-written merely because a player seeks or a row remounts.
    const work = this.writes
      .then(() =>
        maintain(async () => {
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
        }),
      )
      .catch((error: unknown) => {
        if (this.persistedAt.get(key) === now) this.persistedAt.delete(key);
        ignoreStorageFailure(error);
      })
      .finally(() => {
        this.recencyWrites.delete(key);
      });
    this.writes = work.catch(() => {});
    keepAlive(work);
  }
  private async evict(
    cache: MediaCacheStore,
    index: { sizes: Map<string, number>; bytes: number },
    size: number,
  ) {
    const needsSpace = () =>
      index.sizes.size >= entryLimit || index.bytes + size > byteLimit;
    if (!needsSpace()) return;
    const keys = [...index.sizes.keys()];
    let recency: MediaCacheStore | undefined;
    try {
      recency = await this.storage.open(recencyCacheName);
      const records = recency;
      // Read/rank once per insertion, even if a large object evicts many files.
      const timestamps = new Map(
        await Promise.all(
          keys.map(async (key): Promise<[string, number]> => {
            const saved = await records.match(key);
            const value = saved?.headers.get("X-Accessed-At") ?? "";
            const accessed = Number(value);
            const valid =
              saved?.status === 200 &&
              /^\d+$/u.test(value) &&
              Number.isSafeInteger(accessed);
            if (saved && !valid) await records.delete(key);
            return [
              key,
              Math.max(this.touched.get(key) ?? 0, valid ? accessed : 0),
            ];
          }),
        ),
      );
      const ranks = new Map(
        [...this.touched.keys()].map((key, rank) => [key, rank]),
      );
      keys.sort(
        (a, b) =>
          (timestamps.get(a) ?? 0) - (timestamps.get(b) ?? 0) ||
          (ranks.get(a) ?? -1) - (ranks.get(b) ?? -1),
      );
      // Browser eviction and overlapping workers may leave orphaned metadata.
      for (const key of await recency.keys())
        if (!index.sizes.has(key.url)) await recency.delete(key);
    } catch (error) {
      ignoreStorageFailure(error);
      /* Storage failure falls back to insertion order. */
    }
    for (const key of keys) {
      if (!needsSpace()) break;
      await cache.delete(key);
      this.forget(key);
      await recency?.delete(key).catch(ignoreStorageFailure);
    }
  }
  private async download(
    url: URL,
    remote: boolean,
    range: string | null,
    signal: AbortSignal,
  ): Promise<Response> {
    let source = url.href;
    if (remote) {
      const link = new URL(url.pathname.replace(/\/file$/u, "/link"), url);
      if (url.searchParams.get("download") === "true")
        link.searchParams.set("download", "true");
      const response = await this.network(link, {
        credentials: "include",
        cache: "no-store",
        signal,
      });
      if (!response.ok) return new Response(null, { status: response.status });
      const value: unknown = await response.json();
      if (!isFileLink(value, url.href))
        throw new TypeError("Invalid attachment download URL.");
      source = value.url;
    }
    return this.network(source, {
      credentials: remote ? "omit" : "same-origin",
      cache: "no-store",
      mode: "cors",
      headers: range ? { Range: range } : {},
      signal,
    });
  }
  async respond(
    request: Request,
    url: URL,
    keepAlive: (work: Promise<void>) => void,
    remote = false,
  ): Promise<Response> {
    // No offline fallback: logout, expiry, retraction and revoked access apply
    // equally to cached copies. The small info response never touches S3.
    const auth = await this.network(
      new URL(url.pathname.replace(/\/file$/u, "/info"), url),
      {
        credentials: "include",
        cache: "no-store",
        signal: request.signal,
      },
    );
    if (!auth.ok) return new Response(null, { status: auth.status });
    const info: unknown = await auth.json();
    if (!metadata(info))
      throw new TypeError("Invalid attachment metadata response.");
    if (info.expired === true) return new Response(null, { status: 410 });
    const output = headers(info, url.searchParams.get("download") === "true");
    const range = request.headers.get("Range");
    const key = new URL(url.pathname, url);
    const cache = await this.storage.open(cacheName);
    let reservation: Fill | undefined;
    let cancellation: ReturnType<typeof requestAbort> | undefined;
    let saved = await this.read(cache, key.href, info);
    if (!saved && !range) {
      // Reserve before the network await so simultaneous misses share the fill.
      // Every waiter has already performed its own live authorization check.
      const filling = this.fills.get(key.href);
      if (filling) {
        await waitForFill(filling.done, request.signal);
        request.signal.throwIfAborted();
        saved = await this.read(cache, key.href, info);
      } else if (info.size <= byteLimit) {
        reservation = this.reserve(key.href, info.size);
      }
    }
    if (saved) {
      const hit = cachedResponse(saved, output, range, request.signal);
      if (hit) {
        this.touch(key.href, keepAlive);
        return hit;
      }
      saved.body.cancel().catch(() => {});
    }
    try {
      request.signal.throwIfAborted();
      // Once admitted, a bounded whole-object fill survives a virtual row's
      // cancellation. Remounts wait for its persistent bytes instead of restarting.
      if (reservation) cancellation = fillCancellation(request.signal);
      const signal = cancellation?.signal ?? request.signal;
      const response = await this.download(url, remote, range, signal);
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
        reservation &&
        !range &&
        response.status === 200 &&
        size > 0 &&
        this.fillingBytes + size - reservation.size <= fillByteLimit
      ) {
        const target = cache;
        const fill = reservation;
        const deadline = cancellation;
        // Cache only whole objects: seeking cannot fill the cache with overlapping
        // fragments. Serialized writes and recency touches share one order.
        const copy = new Response(result.clone().body, { headers: output });
        this.fillingBytes += size - fill.size;
        fill.size = size;
        reservation = undefined;
        const work = this.writes
          .then(() =>
            maintain(async () => {
              const index = await this.accounting(target);
              // Concurrent misses may already have filled this key.
              await target.delete(key.href);
              this.forget(key.href);
              await this.evict(target, index, size);
              await target.put(key.href, copy);
              this.remember(key.href, size);
              this.touch(key.href, keepAlive);
            }),
          )
          .catch((error: unknown) => {
            this.index = null;
            // Discard the tee branch if Cache rejects before reading its body.
            // Otherwise the continuing download can buffer into an unused copy.
            if (!copy.bodyUsed) copy.body?.cancel().catch(() => {});
            /* Quota/eviction failures must not break a download. */
            ignoreStorageFailure(error);
          });
        this.writes = work.catch(() => {});
        cancellation = undefined;
        keepAlive(
          work.finally(() => {
            deadline?.dispose();
            fill.release();
          }),
        );
      }
      return result;
    } finally {
      cancellation?.dispose();
      reservation?.release();
    }
  }
}

/** Persistent immutable bytes. Every reuse first checks live server authorization. */
const cacheName = "commonroom-media-v2";
const byteLimit = 256 * 1024 * 1024;
const entryLimit = 128;
const touchInterval = 60_000;
interface Metadata {
  size: number;
  content_type: string;
  filename: string;
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
    typeof value.filename === "string"
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
  private readonly touched = new Map<string, number>();
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
  private touch(
    cache: Cache,
    key: string,
    keepAlive: (work: Promise<void>) => void,
  ): void {
    // Keep approximate persisted LRU order without rewriting immutable bytes on
    // every virtual-row remount or player Range request. Authorization stays live.
    const now = Date.now();
    const previous = this.touched.get(key);
    if (previous !== undefined && now - previous < touchInterval) return;
    this.touched.delete(key);
    this.touched.set(key, now);
    if (this.touched.size > entryLimit) {
      const oldest = this.touched.keys().next().value;
      if (oldest !== undefined) this.touched.delete(oldest);
    }
    const work = this.writes
      .then(async () => {
        const saved = await cache.match(key);
        if (!saved) {
          this.forget(key);
          return;
        }
        await cache.delete(key);
        this.forget(key);
        await cache.put(key, saved);
        this.remember(key, Number(saved.headers.get("Content-Length")) || 0);
      })
      .catch(() => {
        this.index = null;
        if (this.touched.get(key) === now) this.touched.delete(key);
        /* Cache maintenance cannot interrupt playback. */
      });
    this.writes = work;
    this.pending.set(key, work);
    keepAlive(
      work.finally(() => {
        if (this.pending.get(key) === work) this.pending.delete(key);
      }),
    );
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
    const output = headers(info, url.searchParams.get("download") === "true");
    const range = request.headers.get("Range");
    const key = new URL(url.pathname, url);
    let cache: Cache | undefined;
    try {
      cache = await this.storage.open(cacheName);
      await this.pending.get(key.href);
      const saved = await cache.match(key.href);
      if (saved && !range) {
        this.touch(cache, key.href, keepAlive);
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
        if (full && match && (match[1] || match[2])) {
          this.touch(cache, key.href, keepAlive);
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
          const blob = await full.blob();
          output.set("Content-Length", String(end - start + 1));
          output.set("Content-Range", `bytes ${start}-${end}/${fullSize}`);
          return new Response(blob.slice(start, end + 1), {
            status: 206,
            headers: output,
          });
        }
      }
    } catch {
      /* Storage-disabled/private browsers still load from the network. */
    }
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
      !range &&
      response.status === 200 &&
      size > 0 &&
      size <= byteLimit
    ) {
      const target = cache;
      // Cache only whole objects: seeking cannot fill the cache with overlapping
      // fragments. Serialized writes and recency touches share one order.
      const copy = new Response(result.clone().body, { headers: output });
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
            const oldest = index.sizes.keys().next().value;
            if (oldest === undefined) break;
            await target.delete(oldest);
            this.forget(oldest);
          }
          await target.put(key.href, copy);
          this.remember(key.href, size);
        })
        .catch(() => {
          this.index = null;
          /* Quota/eviction failures must not break a download. */
        });
      this.writes = work;
      this.pending.set(key.href, work);
      keepAlive(
        work.finally(() => {
          if (this.pending.get(key.href) === work)
            this.pending.delete(key.href);
        }),
      );
    }
    return result;
  }
}

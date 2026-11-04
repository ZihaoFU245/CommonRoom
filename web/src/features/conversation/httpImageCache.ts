import { MediaCache } from "./mediaCache.ts";
import type { MediaCacheStorage, MediaCacheStore } from "./mediaCache.ts";

interface StoredResponse {
  body: Blob;
  headers: Headers;
  status: number;
}

function key(request: RequestInfo | URL): string {
  return request instanceof Request
    ? request.url
    : new URL(String(request)).href;
}

/** Consume streams once; cache hits get independent streams over immutable bytes. */
class MemoryStore implements MediaCacheStore {
  private readonly records = new Map<string, StoredResponse>();

  match(request: RequestInfo | URL): Promise<Response | undefined> {
    const record = this.records.get(key(request));
    return Promise.resolve(
      record
        ? new Response(record.body.size ? record.body : null, {
            headers: record.headers,
            status: record.status,
          })
        : undefined,
    );
  }

  async put(request: RequestInfo | URL, response: Response): Promise<void> {
    const body = await response.blob();
    this.records.set(key(request), {
      body,
      headers: new Headers(response.headers),
      status: response.status,
    });
  }

  delete(request: RequestInfo | URL): Promise<boolean> {
    return Promise.resolve(this.records.delete(key(request)));
  }

  keys(): Promise<readonly Request[]> {
    return Promise.resolve(
      Array.from(this.records.keys(), (url) => new Request(url)),
    );
  }
}

class MemoryStorage implements MediaCacheStorage {
  private readonly stores = new Map<string, MemoryStore>();
  open(name: string): Promise<MemoryStore> {
    let store = this.stores.get(name);
    if (!store) {
      store = new MemoryStore();
      this.stores.set(name, store);
    }
    return Promise.resolve(store);
  }
}

/** Reuse the worker's authorization, fill coalescing and 128-entry/256-MiB budget. */
export class HttpImageCache {
  private readonly cache: MediaCache;
  constructor(network?: typeof fetch) {
    this.cache = new MediaCache(new MemoryStorage(), network);
  }

  invalidate(ids?: readonly string[]): void {
    this.cache.invalidate(ids);
  }

  resolve(href: string, refresh = false, download = false, link = false) {
    return this.cache.resolve(href, refresh, download, link);
  }

  async load(href: string, signal: AbortSignal): Promise<string> {
    const response = await this.cache.respond(
      new Request(href, { signal }),
      new URL(href),
      (work) => {
        work.catch((error: unknown) => {
          console.error("HTTP image cache maintenance failed", error);
        });
      },
    );
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error(`Image request failed (${response.status}).`);
    }
    if (Number(response.headers.get("Content-Length")) > 256 * 1024 * 1024) {
      // Preserve streaming for unusually large images; do not buffer above budget.
      await response.body?.cancel();
      const resolved = await this.cache.resolve(href, false, false, true);
      if (!resolved.url || resolved.info.expired || resolved.info.unavailable)
        throw new Error("Image is no longer available.");
      return resolved.url;
    }
    const body = await response.blob();
    signal.throwIfAborted();
    return URL.createObjectURL(body);
  }
}

export const httpImageCache = new HttpImageCache();

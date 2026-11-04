/** In-memory read leases. Uploads never use this authorization cache. */
import {
  mediaFileUrl,
  mediaFilePath,
  isFileLink,
} from "../../api/mediaFile.ts";
import { requestAbort } from "../../api/requestAbort.ts";

export interface FileInfo {
  media_id: string;
  path: string;
  filename: string;
  content_type: string;
  size: number;
  expires: number | null;
  expired: boolean;
  unavailable?: boolean;
  library?: boolean;
  saved?: boolean;
}
export interface Resolution {
  info: FileInfo;
  expiresAt: number;
  renewAt: number;
  url?: string;
  linkExpiresAt: number;
}
export class MediaAccessError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}
interface Pending {
  href: string;
  base: string;
  kind: "media" | "avatars";
  id: string;
  link: boolean;
  download: boolean;
  started: boolean;
  queued: boolean;
  promise: Promise<Resolution>;
  resolve: (value: Resolution) => void;
  reject: (error: unknown) => void;
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function integer(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
function isFileInfo(value: unknown): value is FileInfo {
  return (
    record(value) &&
    typeof value.media_id === "string" &&
    typeof value.path === "string" &&
    mediaFilePath(value.path)?.id === value.media_id &&
    typeof value.filename === "string" &&
    typeof value.content_type === "string" &&
    integer(value.size) &&
    value.size > 0 &&
    typeof value.expired === "boolean" &&
    (value.expires === null || integer(value.expires)) &&
    [value.unavailable, value.library, value.saved].every(
      (field) => field === undefined || typeof field === "boolean",
    )
  );
}
export function isResolution(value: unknown): value is Resolution {
  return (
    record(value) &&
    isFileInfo(value.info) &&
    integer(value.expiresAt) &&
    integer(value.renewAt) &&
    integer(value.linkExpiresAt) &&
    (value.url === undefined || typeof value.url === "string")
  );
}
export class MediaAccess {
  private readonly entries = new Map<string, Resolution>();
  private readonly links = new Map<
    string,
    { url: string; expiresAt: number }
  >();
  private readonly pending = new Map<string, Pending>();
  private readonly controllers = new Set<AbortController>();
  private readonly lanes = new Map<string, Promise<void>>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private generation = 0;
  private readonly network: typeof fetch;
  constructor(network: typeof fetch = (...args) => fetch(...args)) {
    this.network = network;
  }

  clear(
    ids?: readonly string[],
    reason: unknown = new DOMException("Media access changed", "AbortError"),
  ): void {
    if (ids) {
      const affected = new Set(ids);
      for (const key of this.entries.keys())
        if (affected.has(mediaFileUrl(key)?.id ?? "")) this.forget(key);
      for (const [key, pending] of this.pending) {
        if (!affected.has(pending.id)) continue;
        this.pending.delete(key);
        pending.reject(reason);
      }
      return;
    }
    this.generation++;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.entries.clear();
    this.links.clear();
    for (const controller of this.controllers) controller.abort();
    for (const pending of this.pending.values()) pending.reject(reason);
    this.pending.clear();
    this.lanes.clear();
  }
  private forget(canonical: string): void {
    this.entries.delete(canonical);
    this.links.delete(canonical);
    this.links.delete(`${canonical}?download=true`);
  }
  async load(
    href: string,
    link = false,
    refresh = false,
    download = false,
  ): Promise<Resolution> {
    const file = mediaFileUrl(href);
    if (!file) throw new TypeError("Invalid managed media URL.");
    const canonical = new URL(file.url.pathname, file.url).href;
    const key = canonical + (download ? "?download=true" : "");
    const cached = this.entries.get(canonical);
    const signed = this.links.get(key);
    if (
      !refresh &&
      cached &&
      cached.expiresAt > Date.now() &&
      (!link ||
        cached.info.expired ||
        cached.info.unavailable ||
        (signed && signed.expiresAt > Date.now()))
    ) {
      this.entries.delete(canonical);
      this.entries.set(canonical, cached);
      return link && signed && !cached.info.expired && !cached.info.unavailable
        ? {
            ...cached,
            url: signed.url,
            linkExpiresAt: signed.expiresAt,
          }
        : cached;
    }
    let existing = this.pending.get(key);
    if (existing) {
      if (link && !existing.link) {
        if (!existing.started) existing.link = true;
        else {
          await existing.promise;
          return this.load(href, link, false, download);
        }
      }
      return existing.promise;
    }
    if (this.pending.size >= 256)
      throw new MediaAccessError(
        429,
        "Media reads are busy. Try again shortly.",
      );
    let resolve: Pending["resolve"] = () => {};
    let reject: Pending["reject"] = () => {};
    const promise = new Promise<Resolution>((accept, fail) => {
      resolve = accept;
      reject = fail;
    });
    existing = {
      href: key,
      base: file.base,
      kind: file.kind,
      id: file.id,
      link,
      download,
      started: false,
      queued: false,
      promise,
      resolve,
      reject,
    };
    this.pending.set(key, existing);
    this.timer ??= setTimeout(() => {
      this.timer = undefined;
      this.flush();
    }, 5);
    return promise;
  }
  private flush(): void {
    const groups = new Map<string, Pending[]>();
    for (const item of this.pending.values()) {
      if (item.queued) continue;
      item.queued = true;
      const group = groups.get(item.base) ?? [];
      group.push(item);
      groups.set(item.base, group);
    }
    const generation = this.generation;
    for (const [base, group] of groups) {
      const previous = this.lanes.get(base) ?? Promise.resolve();
      const work = previous.then(async () => {
        if (generation === this.generation) await this.sendGroups(base, group);
      });
      this.lanes.set(base, work);
      const finish = () => {
        if (this.lanes.get(base) === work) this.lanes.delete(base);
      };
      work.then(finish, (error: unknown) => {
        for (const item of group) item.reject(error);
        finish();
      });
    }
  }

  private async sendGroups(base: string, group: Pending[]): Promise<void> {
    // Serialize batches to stay within account admission limits.
    const generation = this.generation;
    for (let offset = 0; offset < group.length; offset += 32) {
      if (generation !== this.generation) return;
      await this.send(
        base,
        group
          .slice(offset, offset + 32)
          .filter((item) => this.pending.get(item.href) === item),
      );
    }
  }
  private async send(
    base: string,
    items: Pending[],
    retry = true,
  ): Promise<void> {
    if (!items.length) return;
    const generation = this.generation;
    for (const item of items) item.started = true;
    const started = Date.now();
    const controller = new AbortController();
    this.controllers.add(controller);
    const cancellation = requestAbort(15000, controller.signal);
    try {
      const response = await this.network(new URL("api/media/resolve", base), {
        method: "POST",
        credentials: "include",
        cache: "default",
        signal: cancellation.signal,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          items: items.map(({ kind, id, link, download }) => ({
            kind,
            id,
            link,
            download,
          })),
        }),
      });
      if (response.status === 409 && retry) {
        await response.body?.cancel();
        cancellation.signal.throwIfAborted();
        if (generation === this.generation)
          await this.send(
            base,
            items.filter((item) => this.pending.get(item.href) === item),
            false,
          );
        return;
      }
      if (!response.ok)
        throw new MediaAccessError(
          response.status,
          `Media resolution failed (${response.status}).`,
        );
      const value: unknown = await response.json();
      cancellation.signal.throwIfAborted();
      if (generation !== this.generation)
        throw new DOMException("Media access changed", "AbortError");
      if (
        !record(value) ||
        !Array.isArray(value.items) ||
        value.items.length !== items.length
      )
        throw new TypeError("Invalid media resolution response.");
      for (const [index, item] of items.entries()) {
        if (this.pending.get(item.href) !== item) continue;
        const result: unknown = value.items[index];
        if (
          !record(result) ||
          result.id !== item.id ||
          result.kind !== item.kind ||
          !integer(result.status)
        )
          throw new TypeError("Invalid media resolution item.");
        if (result.status !== 200) {
          this.forget(new URL(new URL(item.href).pathname, item.href).href);
          item.reject(
            new MediaAccessError(
              result.status,
              typeof result.error === "string"
                ? result.error
                : "Media unavailable.",
            ),
          );
          continue;
        }
        if (
          !isFileInfo(result.info) ||
          result.info.media_id !== item.id ||
          new URL(result.info.path, base).href !==
            new URL(new URL(item.href).pathname, base).href ||
          !integer(result.lease_seconds) ||
          result.lease_seconds === 0 ||
          result.lease_seconds > 86400
        )
          throw new TypeError("Invalid media authorization lease.");
        let url: string | undefined;
        let linkExpiresAt = 0;
        if (result.link !== undefined) {
          const signed = result.link;
          if (
            !record(signed) ||
            !integer(signed.expires_in) ||
            signed.expires_in === 0 ||
            signed.expires_in > 60
          )
            throw new TypeError("Invalid media signed link.");
          const lifetime = signed.expires_in;
          if (!isFileLink(signed, base))
            throw new TypeError("Invalid media signed link.");
          url = signed.url;
          linkExpiresAt =
            started +
            Math.max(0, lifetime * 1000 - Math.min(1000, lifetime * 500));
        }
        const expiresAt = Math.min(
          started + result.lease_seconds * 1000,
          result.info.expired
            ? Infinity
            : (result.info.expires ?? Infinity) * 1000,
        );
        const resolution: Resolution = {
          info: result.info,
          expiresAt,
          renewAt: started + Math.floor(result.lease_seconds * 1000 * 0.8),
          linkExpiresAt,
          ...(url === undefined ? {} : { url }),
        };
        if (url) this.links.set(item.href, { url, expiresAt: linkExpiresAt });
        this.entries.delete(
          new URL(new URL(item.href).pathname, item.href).href,
        );
        this.entries.set(
          new URL(new URL(item.href).pathname, item.href).href,
          resolution,
        );
        while (this.entries.size > 256) {
          const oldest = this.entries.keys().next().value;
          if (oldest !== undefined) this.forget(oldest);
        }
        item.resolve(resolution);
      }
    } catch (error) {
      if (error instanceof MediaAccessError && error.status === 401)
        this.clear(undefined, error);
      for (const item of items) item.reject(error);
    } finally {
      cancellation.dispose();
      this.controllers.delete(controller);
      for (const item of items)
        if (this.pending.get(item.href) === item)
          this.pending.delete(item.href);
    }
  }
}

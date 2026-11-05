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
  file: NonNullable<ReturnType<typeof mediaFileUrl>>;
  key: string;
  link: boolean;
  download: boolean;
  state: "waiting" | "queued" | "started";
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
      for (const [key, value] of this.entries)
        if (affected.has(value.info.media_id)) this.forget(key);
      for (const [key, pending] of this.pending) {
        if (!affected.has(pending.file.id)) continue;
        this.pending.delete(key);
        pending.reject(reason);
      }
      return;
    }
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
    file.url.search = "";
    const canonical = file.url.href;
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
        if (existing.state !== "started") existing.link = true;
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
      file,
      key,
      link,
      download,
      state: "waiting",
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
      if (item.state !== "waiting") continue;
      item.state = "queued";
      const group = groups.get(item.file.base) ?? [];
      group.push(item);
      groups.set(item.file.base, group);
    }
    for (const [base, group] of groups) {
      const previous = this.lanes.get(base) ?? Promise.resolve();
      const work = previous.then(() => this.sendGroups(base, group));
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
    for (let offset = 0; offset < group.length; offset += 32) {
      await this.send(
        base,
        group
          .slice(offset, offset + 32)
          .filter((item) => this.pending.get(item.key) === item),
      );
    }
  }
  private async send(base: string, items: Pending[]): Promise<void> {
    if (!items.length) return;
    for (const item of items) item.state = "started";
    const controller = new AbortController();
    this.controllers.add(controller);
    const cancellation = requestAbort(15000, controller.signal);
    try {
      for (let attempt = 0; ; attempt++) {
        const started = Date.now();
        const response = await this.network(
          new URL("api/media/resolve", base),
          {
            method: "POST",
            credentials: "include",
            cache: "default",
            signal: cancellation.signal,
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              items: items.map(({ file: { kind, id }, link, download }) => ({
                kind,
                id,
                link,
                download,
              })),
            }),
          },
        );
        if (response.status === 409 && attempt === 0) {
          await response.body?.cancel();
          cancellation.signal.throwIfAborted();
          items = items.filter((item) => this.pending.get(item.key) === item);
          if (!items.length) return;
          continue;
        }
        if (!response.ok)
          throw new MediaAccessError(
            response.status,
            `Media resolution failed (${response.status}).`,
          );
        const value: unknown = await response.json();
        cancellation.signal.throwIfAborted();
        if (
          !record(value) ||
          !Array.isArray(value.items) ||
          value.items.length !== items.length
        )
          throw new TypeError("Invalid media resolution response.");
        const packet = value.items;
        // Validate the entire packet before installing leases or settling readers.
        const results = items.map((item, index) => {
          const result: unknown = packet[index];
          if (
            !record(result) ||
            result.id !== item.file.id ||
            result.kind !== item.file.kind ||
            !integer(result.status)
          )
            throw new TypeError("Invalid media resolution item.");
          if (result.status !== 200) {
            return [
              item,
              new MediaAccessError(
                result.status,
                typeof result.error === "string"
                  ? result.error
                  : "Media unavailable.",
              ),
            ] as const;
          }
          if (
            !isFileInfo(result.info) ||
            result.info.media_id !== item.file.id ||
            new URL(result.info.path, base).href !== item.file.url.href ||
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
          return [item, resolution] as const;
        });
        for (const [item, resolution] of results) {
          if (this.pending.get(item.key) !== item) continue;
          if (resolution instanceof MediaAccessError) {
            this.forget(item.file.url.href);
            item.reject(resolution);
            continue;
          }
          if (resolution.url)
            this.links.set(item.key, {
              url: resolution.url,
              expiresAt: resolution.linkExpiresAt,
            });
          this.entries.delete(item.file.url.href);
          this.entries.set(item.file.url.href, resolution);
          while (this.entries.size > 256) {
            const oldest = this.entries.keys().next().value;
            if (oldest !== undefined) this.forget(oldest);
          }
          item.resolve(resolution);
        }
        return;
      }
    } catch (error) {
      if (error instanceof MediaAccessError && error.status === 401)
        this.clear(undefined, error);
      for (const item of items) item.reject(error);
    } finally {
      cancellation.dispose();
      this.controllers.delete(controller);
      for (const item of items)
        if (this.pending.get(item.key) === item) this.pending.delete(item.key);
    }
  }
}

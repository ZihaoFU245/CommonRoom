import type { MediaInfo } from "../../api/media.ts";

interface Entry {
  info?: MediaInfo;
  pending?: Promise<MediaInfo>;
  controller?: AbortController;
}

// Presentation metadata only. Never use these records to authorize file bytes.
// Keep in-flight ownership here so virtual unmounts cannot restart the request.
class MediaStatusCache {
  private readonly entries = new Map<string, Entry>();

  clear() {
    for (const entry of this.entries.values()) entry.controller?.abort();
    this.entries.clear();
  }

  peek(key: string): MediaInfo | undefined {
    const info = this.entries.get(key)?.info;
    return info &&
      (info.expired ||
        info.expires === null ||
        info.expires * 1000 > Date.now())
      ? info
      : undefined;
  }

  load(
    key: string,
    fetchInfo: (signal: AbortSignal) => Promise<MediaInfo>,
    refresh = false,
  ): Promise<MediaInfo> {
    const previous = this.entries.get(key);
    if (previous?.pending) return previous.pending;
    const info = this.peek(key);
    if (!refresh && info) {
      this.entries.delete(key);
      this.entries.set(key, { info });
      return Promise.resolve(info);
    }
    if (!previous && this.entries.size >= 128) {
      // Never evict a pending request: that loses ownership and permits duplicate
      // background loads. Decline excess cold checks until capacity is available.
      let oldest: string | undefined;
      for (const [key, entry] of this.entries) {
        if (!entry.pending) {
          oldest = key;
          break;
        }
      }
      if (oldest === undefined)
        return Promise.reject(
          new Error("Attachment status checks are busy. Try again shortly."),
        );
      this.entries.delete(oldest);
    }
    const controller = new AbortController();
    const entry: Entry = previous?.info
      ? { info: previous.info, controller }
      : { controller };
    const pending = fetchInfo(controller.signal).then(
      (value) => {
        if (this.entries.get(key) === entry) {
          delete entry.pending;
          delete entry.controller;
          entry.info = value;
        }
        return value;
      },
      (error: unknown) => {
        if (this.entries.get(key) === entry) this.entries.delete(key);
        throw error;
      },
    );
    entry.pending = pending;
    this.entries.delete(key);
    this.entries.set(key, entry);
    return pending;
  }
}

export const mediaStatusCache = new MediaStatusCache();

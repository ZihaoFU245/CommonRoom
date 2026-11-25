import {
  durationBucket,
  maxMetricCount,
  mediaBucketCounts,
  mediaMetrics,
  type MediaMetric,
} from "./api/clientMetricSchema.ts";
import { MediaCache } from "./features/conversation/mediaCache.ts";
import { mediaFileUrl, mediaRequestUrl } from "./api/mediaFile.ts";
import {
  isPushPayload,
  notificationTagPrefix,
} from "./features/preferences/pushPayload.ts";
// Keep the worker isolated from Window globals and application bootstrap.
interface Lifetime {
  waitUntil(work: Promise<unknown>): void;
}
interface MediaFetch extends Lifetime {
  request: Request;
  clientId: string;
  respondWith(response: Promise<Response>): void;
}
interface MediaClient {
  id: string;
  postMessage(data: unknown): void;
}
interface MediaMessage extends Lifetime {
  data: unknown;
  ports: readonly MessagePort[];
  source: MediaClient | null;
}
interface ChatWindow {
  url: string;
  navigate(url: string): Promise<ChatWindow | null>;
  focus(): Promise<ChatWindow>;
}
declare const self: {
  registration: Pick<ServiceWorkerRegistration, "scope" | "showNotification">;
  clients: {
    claim(): Promise<void>;
    matchAll(options: { type: "window" }): Promise<ChatWindow[]>;
    openWindow(url: string): Promise<ChatWindow | null>;
  };
  skipWaiting(): Promise<void>;
  addEventListener(
    type: "install" | "activate",
    listener: (event: Lifetime) => void,
  ): void;
  addEventListener(type: "fetch", listener: (event: MediaFetch) => void): void;
  addEventListener(
    type: "message",
    listener: (event: MediaMessage) => void,
  ): void;
  addEventListener(
    type: "push",
    listener: (event: Lifetime & { data: { json(): unknown } | null }) => void,
  ): void;
  addEventListener(
    type: "notificationclick",
    listener: (event: Lifetime & { notification: Notification }) => void,
  ): void;
};
self.addEventListener("push", (event) => {
  let data: unknown;
  try {
    data = event.data?.json();
  } catch {
    // A malformed payload can still produce a generic notification.
  }
  const payload = isPushPayload(data) ? data : undefined;
  event.waitUntil(
    self.registration.showNotification(
      payload?.title || "CommonRoom",
      payload
        ? {
            body: payload.body || "New message",
            tag: notificationTagPrefix + payload.id,
            data: { id: payload.id, view: payload.view },
          }
        : { body: "New message", tag: notificationTagPrefix },
    ),
  );
});

self.addEventListener("notificationclick", (event) => {
  if (!event.notification.tag.startsWith(notificationTagPrefix)) return;
  event.notification.close();
  const data: unknown = event.notification.data;
  const target = new URL(self.registration.scope);
  if (isPushPayload(data))
    target.hash = new URLSearchParams({ room: data.view }).toString();
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: "window" });
      for (const client of windows) {
        const current = new URL(client.url);
        if (current.pathname !== target.pathname) continue;
        if (current.href === target.href) {
          // eslint-disable-next-line no-restricted-syntax -- This focuses a browser window.
          await client.focus();
          return;
        }
        const navigated = await client.navigate(target.href).catch(() => null);
        if (navigated) {
          // eslint-disable-next-line no-restricted-syntax -- This focuses a browser window.
          await navigated.focus();
          return;
        }
      }
      await self.clients.openWindow(target.href);
    })(),
  );
});

const media = new MediaCache(caches);
const metricClients = new Map<
  string,
  {
    base: string;
    epoch: number;
    counts: number[][];
    totals: number[];
    dirty: boolean;
    observe: (metric: MediaMetric, started?: number) => void;
  }
>();

function metricMessage(
  event: MediaMessage,
  data: object & { kind: unknown },
): boolean {
  if (
    typeof data.kind !== "string" ||
    !data.kind.startsWith("commonroom-metrics:")
  )
    return false;
  const client = event.source;
  if (!client?.id || !("epoch" in data) || typeof data.epoch !== "number")
    return true;
  const current = metricClients.get(client.id);
  if (data.kind === "commonroom-metrics:stop") {
    if (current?.epoch === data.epoch) metricClients.delete(client.id);
    return true;
  }
  if (
    !("base" in data) ||
    typeof data.base !== "string" ||
    data.base.length > 1024
  )
    return true;
  if (data.kind === "commonroom-metrics:start") {
    if (current?.epoch === data.epoch && current.base === data.base)
      return true;
    if (metricClients.size >= 64) {
      const oldest = metricClients.keys().next().value;
      if (oldest !== undefined) metricClients.delete(oldest);
    }
    const epoch = data.epoch;
    const counts = mediaBucketCounts.map((length) =>
      Array<number>(length).fill(0),
    );
    const totals = mediaMetrics.map(() => 0);
    const entry = {
      base: data.base,
      epoch,
      counts,
      totals,
      dirty: false,
      observe(metric: MediaMetric, started?: number) {
        if (metricClients.get(client.id) !== entry) return;
        const index = mediaMetrics.indexOf(metric);
        const count = totals[index] ?? 0;
        const row = counts[index];
        if (!row || count >= maxMetricCount) return;
        totals[index] = count + 1;
        const bucket =
          started === undefined
            ? 0
            : durationBucket(performance.now() - started);
        row[bucket] = (row[bucket] ?? 0) + 1;
        if (!entry.dirty) {
          entry.dirty = true;
          try {
            client.postMessage(`commonroom-metrics:dirty:${epoch}`);
          } catch {
            metricClients.delete(client.id);
          }
        }
      },
    };
    metricClients.set(client.id, entry);
  } else if (data.kind === "commonroom-metrics:read") {
    const port = event.ports[0];
    if (!port) return true;
    if (current?.epoch === data.epoch && current.base === data.base) {
      port.postMessage(current.counts);
      for (const row of current.counts) row.fill(0);
      current.totals.fill(0);
      current.dirty = false;
    } else {
      port.postMessage(null);
    }
    port.close();
  }
  return true;
}
self.addEventListener("message", (event) => {
  const data = event.data;
  if (data && typeof data === "object" && "kind" in data) {
    if (metricMessage(event, data)) return;
    if (data.kind === "commonroom-media:clear") {
      const ids =
        "ids" in data &&
        Array.isArray(data.ids) &&
        data.ids.every((id: unknown) => typeof id === "string")
          ? data.ids
          : undefined;
      media.invalidate(ids);
      event.ports[0]?.postMessage({ ok: true });
      event.ports[0]?.close();
      return;
    }
    if (
      data.kind === "commonroom-media:resolve" &&
      "href" in data &&
      typeof data.href === "string" &&
      mediaFileUrl(data.href)
    ) {
      const port = event.ports[0];
      event.waitUntil(
        media
          .resolve(
            data.href,
            "refresh" in data && data.refresh === true,
            "download" in data && data.download === true,
            "link" in data && data.link === true,
          )
          .then(
            (value) => port?.postMessage({ value }),
            (error: unknown) =>
              port?.postMessage({
                error:
                  error instanceof Error
                    ? error.message
                    : "Media resolution failed.",
                status:
                  error &&
                  typeof error === "object" &&
                  "status" in error &&
                  typeof error.status === "number"
                    ? error.status
                    : 503,
              }),
          )
          .finally(() => port?.close()),
      );
      return;
    }
  }
  if (data !== "commonroom-media:claim") return;
  event.waitUntil(
    (async () => {
      await self.clients.claim();
      event.ports[0]?.postMessage("commonroom-media:ready");
      event.ports[0]?.close();
    })(),
  );
});
self.addEventListener("install", (event) =>
  event.waitUntil(self.skipWaiting()),
);
self.addEventListener("activate", (event) =>
  event.waitUntil(self.clients.claim()),
);
self.addEventListener("fetch", (event) => {
  const url = mediaRequestUrl(event.request, self.registration.scope);
  if (url) {
    const metrics = metricClients.get(event.clientId);
    event.respondWith(
      media.respond(
        event.request,
        url,
        (work) => event.waitUntil(work),
        true,
        metrics && metrics.base === mediaFileUrl(url.href)?.base
          ? metrics.observe
          : undefined,
      ),
    );
  }
});

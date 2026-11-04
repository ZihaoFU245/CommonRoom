import { MediaCache } from "./features/conversation/mediaCache.ts";
import { mediaFileUrl, mediaRequestUrl } from "./api/mediaFile.ts";
// Keep the worker isolated from Window globals and application bootstrap.
interface Lifetime {
  waitUntil(work: Promise<unknown>): void;
}
interface MediaFetch extends Lifetime {
  request: Request;
  respondWith(response: Promise<Response>): void;
}
interface MediaMessage extends Lifetime {
  data: unknown;
  ports: readonly MessagePort[];
}
declare const self: {
  registration: { scope: string };
  clients: { claim(): Promise<void> };
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
};
const media = new MediaCache(caches);
self.addEventListener("message", (event) => {
  const data = event.data;
  if (data && typeof data === "object" && "kind" in data) {
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
  if (url)
    event.respondWith(
      media.respond(event.request, url, (work) => event.waitUntil(work)),
    );
});

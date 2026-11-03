import { MediaCache } from "./features/conversation/mediaCache.ts";
import { mediaRequestUrl } from "./api/mediaFile.ts";
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
const origin = new URL(self.registration.scope).origin;
self.addEventListener("message", (event) => {
  if (event.data !== "commonroom-media:claim") return;
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
      media.respond(
        event.request,
        url,
        (work) => event.waitUntil(work),
        url.origin !== origin,
      ),
    );
});

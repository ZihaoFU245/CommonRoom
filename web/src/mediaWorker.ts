import {
  MediaCache,
  mediaCachePath,
} from "./features/conversation/mediaCache.ts";
// Keep the worker isolated from Window globals and application bootstrap.
interface Lifetime {
  waitUntil(work: Promise<unknown>): void;
}
interface MediaFetch extends Lifetime {
  request: Request;
  respondWith(response: Promise<Response>): void;
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
};
const media = new MediaCache(caches);
self.addEventListener("install", (event) =>
  event.waitUntil(self.skipWaiting()),
);
self.addEventListener("activate", (event) =>
  event.waitUntil(
    (async () => {
      await caches.delete("commonroom-media-v1").catch(() => {
        /* Storage-disabled browsers still need worker/network fallback. */
      });
      await self.clients.claim();
    })(),
  ),
);
self.addEventListener("fetch", (event) => {
  const url = mediaCachePath(event.request, self.registration.scope);
  if (url)
    event.respondWith(
      media.respond(event.request, url, (work) => event.waitUntil(work)),
    );
});

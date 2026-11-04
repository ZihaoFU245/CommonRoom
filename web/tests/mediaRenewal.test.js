import assert from "node:assert/strict";
import test from "node:test";
import {
  watchMediaRead,
  invalidateMediaReads,
} from "../src/features/conversation/mediaController.ts";
import { mediaResolution } from "./helpers/mediaResolve.js";
const href =
  "http://api.test/chat/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check) {
  const deadline = Date.now() + 2500;
  while (!check()) {
    assert.ok(Date.now() < deadline, "background renewal did not run");
    await sleep(5);
  }
}

test("mounted media share renewal; hidden readers stop renewal", async (t) => {
  const descriptors = Object.fromEntries(
    ["document", "navigator"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  const page = new EventTarget();
  page.baseURI = "http://ui.test/chat/";
  page.hidden = false;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: page,
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {},
  });
  let calls = 0;
  t.mock.method(Math, "random", () => 0);
  t.mock.method(globalThis, "fetch", async (url, options) => {
    calls++;
    const packet = await mediaResolution(url, options).json();
    for (const item of packet.items) item.lease_seconds = 1;
    return Response.json(packet);
  });
  let first = () => {},
    second = () => {};
  t.after(async () => {
    first();
    second();
    await invalidateMediaReads();
    for (const [key, descriptor] of Object.entries(descriptors)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  await invalidateMediaReads();
  first = watchMediaRead(href);
  second = watchMediaRead(href);
  await until(() => calls === 1);
  first();
  first = () => {};
  await until(() => calls === 2);
  assert.equal(
    calls,
    2,
    "renewal needs no byte request and duplicate mounts share it",
  );
  page.hidden = true;
  page.dispatchEvent(new Event("visibilitychange"));
  await sleep(1100);
  assert.equal(calls, 2, "hidden pages do not renew");
  page.hidden = false;
  page.dispatchEvent(new Event("visibilitychange"));
  await until(() => calls === 3);
  second();
  second = () => {};
  await sleep(900);
  assert.equal(calls, 3, "the last unmount releases renewal ownership");
});

test("distinct mounted assets renew in one heartbeat batch", async (t) => {
  const descriptors = Object.fromEntries(
    ["document", "navigator"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  const page = new EventTarget();
  page.baseURI = "http://ui.test/chat/";
  page.hidden = false;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: page,
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {},
  });
  const batches = [];
  t.mock.method(globalThis, "fetch", async (url, options) => {
    batches.push(JSON.parse(options.body).items.length);
    const packet = await mediaResolution(url, options).json();
    for (const item of packet.items) item.lease_seconds = 1;
    return Response.json(packet);
  });
  const releases = [];
  t.after(async () => {
    for (const release of releases) release();
    await invalidateMediaReads();
    for (const [key, descriptor] of Object.entries(descriptors)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  await invalidateMediaReads();
  for (let index = 0; index < 3; index++)
    releases.push(
      watchMediaRead(href.replace("d5cd8a44", String(index).padStart(8, "0"))),
    );
  await until(() => batches.length === 2);
  assert.deepEqual(batches, [3, 3]);
});

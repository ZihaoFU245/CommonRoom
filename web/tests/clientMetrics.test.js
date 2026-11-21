import assert from "node:assert/strict";
import { test } from "node:test";
import { clientCohort } from "../src/api/clientEnvironment.ts";
import { metric } from "../src/api/clientMetricSchema.ts";

test("sampled browser and worker metrics flush together at most once per minute", async (t) => {
  let stop;
  t.after(() => stop?.());
  let now = 1000;
  const batches = [];
  const document = Object.assign(new EventTarget(), { hidden: false });
  let epoch;
  let reads = 0;
  const controller = {
    postMessage(message, ports) {
      if (message.kind === "commonroom-metrics:start") epoch = message.epoch;
      if (message.kind === "commonroom-metrics:read") {
        reads++;
        ports[0].postMessage([
          [0, 2, 0, 0, 0, 0, 0, 0],
          [0, 0, 1, 0, 0, 0, 0, 0],
          [3],
          [1],
          [0],
        ]);
        ports[0].close();
      }
    },
  };
  const serviceWorker = Object.assign(new EventTarget(), { controller });
  for (const [name, value] of Object.entries({
    window: new EventTarget(),
    document,
    navigator: {
      serviceWorker,
      userAgent:
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/128.0.0.0",
      platform: "Linux x86_64",
      maxTouchPoints: 0,
    },
    matchMedia: (query) => ({ matches: query === "(display-mode: browser)" }),
    performance: { now: () => now },
    fetch: (url, options) => {
      batches.push({
        url: url.href,
        ...options,
        body: JSON.parse(options.body),
      });
      return Promise.reject(new Error("offline"));
    },
  })) {
    const previous = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => {
      if (previous) Object.defineProperty(globalThis, name, previous);
      else Reflect.deleteProperty(globalThis, name);
    });
  }
  t.mock.method(Math, "random", () => 0);
  const api = await import("../src/api/clientMetrics.ts");
  stop = api.stopClientMetrics;
  api.applyClientMetricsPolicy("https://remote.example/chat/", "25, 60");

  const started = api.clientMetricClock();
  now += 75;
  api.recordClientDuration(metric.uploadPut, started);
  for (let i = 0; i < 300; i++)
    api.recordClientMetric(metric.mediaConfigReused);

  serviceWorker.dispatchEvent(
    Object.assign(new Event("message"), {
      source: controller,
      data: `commonroom-metrics:dirty:${epoch}`,
    }),
  );
  assert.equal(batches.length, 0);
  document.hidden = true;
  document.dispatchEvent(new Event("visibilitychange"));
  for (let i = 0; i < 20 && !batches.length; i++)
    await new Promise((resolve) => setImmediate(resolve));

  assert.equal(reads, 1);
  assert.equal(batches.length, 1);
  assert.equal(batches[0].url, "https://remote.example/chat/api/drop");
  assert.equal(batches[0].credentials, "include");
  assert.equal(batches[0].keepalive, true);
  assert.equal(batches[0].priority, "low");
  assert.equal(batches[0].headers, undefined);
  assert.equal(batches[0].body.v, 2);
  assert.equal(batches[0].body.c, 16);
  assert.ok(Number.isSafeInteger(batches[0].body.id));
  assert.deepEqual(batches[0].body.rows, [
    [metric.uploadPut, [0, 0, 1, 0, 0, 0, 0, 0]],
    [metric.mediaBytesReused, [0, 2, 0, 0, 0, 0, 0, 0]],
    [metric.mediaBytesFetched, [0, 0, 1, 0, 0, 0, 0, 0]],
    [metric.mediaLeaseReused, [3]],
    [metric.mediaLeaseResolved, [1]],
    [metric.mediaConfigReused, [256]],
  ]);

  api.recordClientMetric(metric.uploadCancelled);
  api.recordFirstClientDuration(metric.firstSend, now - 35);
  api.recordFirstClientDuration(metric.firstSend, now - 240);
  window.dispatchEvent(new Event("unhandledrejection"));
  document.dispatchEvent(new Event("visibilitychange"));
  assert.equal(batches.length, 1);
  now += 60_000;
  document.dispatchEvent(new Event("visibilitychange"));
  assert.equal(batches.length, 2);
  assert.notEqual(batches[0].body.id, batches[1].body.id);
  assert.deepEqual(batches[1].body.rows, [
    [metric.uploadCancelled, [1]],
    [metric.firstSend, [0, 1, 0, 0, 0, 0, 0, 0]],
    [metric.promiseError, [1]],
  ]);
});

test("client cohorts stay coarse across desktop, phone, tablet and app mode", () => {
  const cohort = (userAgent, platform, maxTouchPoints, display, standalone) =>
    clientCohort({ userAgent, platform, maxTouchPoints, standalone }, display);
  assert.equal(cohort("Unknown", "", 0, "unknown"), 0);
  assert.equal(
    cohort("X11 Gecko/20100101 Firefox/130.0", "Linux x86_64", 0, "browser"),
    40,
  );
  assert.equal(
    cohort(
      "iPhone AppleWebKit/605.1.15 CriOS/128.0 Mobile",
      "iPhone",
      5,
      "browser",
    ),
    31,
  );
  assert.equal(
    cohort(
      "Macintosh AppleWebKit/605.1.15 Safari/605.1.15",
      "MacIntel",
      5,
      "unknown",
      true,
    ),
    35,
  );
  assert.equal(
    cohort(
      "Android AppleWebKit/537.36 Chrome/128.0.0.0 Mobile",
      "Linux arm",
      5,
      "standalone",
    ),
    20,
  );
});

test("missing, disabled and below-draw policies skip collection", async (t) => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    value: {},
    configurable: true,
  });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else Reflect.deleteProperty(globalThis, "window");
  });
  t.mock.method(Math, "random", () => 0.5);
  const api = await import("../src/api/clientMetrics.ts?unsampled");
  for (const policy of [null, "0, 2", "25, 60", "101, 60", "100, 1"]) {
    api.applyClientMetricsPolicy("https://remote.example/chat/", policy);
    assert.equal(api.clientMetricClock(), undefined);
  }
});

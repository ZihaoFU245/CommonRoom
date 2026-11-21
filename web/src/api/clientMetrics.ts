import { clientCohort } from "./clientEnvironment.ts";
import {
  type ClientMetric,
  durationBucket,
  durationBucketCount,
  mediaBucketCounts,
  maxMetricCount,
  mediaMetrics,
  metric,
} from "./clientMetricSchema.ts";

const interval = 60_000;
const maxBatchCount = 1024;
const sampleDraw = typeof window === "undefined" ? 1 : Math.random();
const openedInForeground = typeof document !== "undefined" && !document.hidden;
type Tab = {
  endpoint: URL;
  base: string;
  firstDelay: number;
  epoch: number;
  rows: Map<ClientMetric, { buckets: number[]; count: number }>;
  count: number;
  cohort: number;
  resizeNotice: boolean;
  firsts: Set<ClientMetric>;
  chatStart: number;
  firstForeground: boolean;
  signedIn: boolean;
  conversationStart: number | undefined;
  stickerStart: number | undefined;
  observer?: IntersectionObserver;
  candidates: Map<Element, { metric: ClientMetric; started: number }>;
  dispose: () => void;
  workerDirty: boolean;
  registerWorker?: () => void;
  timer: ReturnType<typeof setTimeout> | undefined;
};
let tab: Tab | undefined;
let epoch = 0;
let lastFlush = -interval;
let sending = false;
let signIn: { base: string; started: number } | undefined;

export function applyClientMetricsPolicy(
  base: string,
  policy: string | null,
): void {
  const match = /^(100|[1-9]?\d), (2|60)$/u.exec(policy ?? "");
  if (!match || sampleDraw >= Number(match[1]) / 100) {
    tab?.dispose();
    return;
  }
  const firstDelay = Number(match[2]) * 1000;
  const signedInAt = signIn?.base === base ? signIn.started : undefined;
  signIn = undefined;
  if (tab?.base === base) {
    tab.firstDelay = firstDelay;
    if (signedInAt !== undefined) {
      tab.chatStart = signedInAt;
      tab.signedIn = true;
    }
    return;
  }
  tab?.dispose();
  const display =
    typeof matchMedia === "function"
      ? matchMedia(
          "(display-mode: standalone), (display-mode: fullscreen), (display-mode: minimal-ui)",
        ).matches
        ? "standalone"
        : matchMedia("(display-mode: browser)").matches
          ? "browser"
          : "unknown"
      : "unknown";
  const state: Tab = {
    endpoint: new URL("api/drop", base),
    base,
    firstDelay,
    epoch: ++epoch,
    rows: new Map(),
    count: 0,
    cohort: clientCohort(navigator, display),
    resizeNotice: false,
    firsts: new Set(),
    chatStart: signedInAt ?? (openedInForeground ? 0 : performance.now()),
    firstForeground: !document.hidden,
    signedIn: signedInAt !== undefined,
    conversationStart: undefined,
    stickerStart: undefined,
    candidates: new Map(),
    dispose: () => {},
    workerDirty: false,
    timer: undefined,
  };
  tab = state;
  const worker = navigator.serviceWorker;
  const register = () =>
    safeWorkerMessage(worker?.controller, {
      kind: "commonroom-metrics:start",
      base,
      epoch: state.epoch,
    });
  state.registerWorker = register;
  const dirty = (event: MessageEvent<unknown>) => {
    if (
      event.source === worker?.controller &&
      event.data === `commonroom-metrics:dirty:${state.epoch}`
    ) {
      state.workerDirty = true;
      schedule(state);
    }
  };
  const scriptError = (event: ErrorEvent) => {
    if (!(event instanceof ErrorEvent)) return;
    if (
      event.message ===
        "ResizeObserver loop completed with undelivered notifications." ||
      event.message === "ResizeObserver loop limit exceeded"
    ) {
      if (!state.resizeNotice) {
        state.resizeNotice = true;
        recordClientMetric(metric.browserResizeNotice);
      }
      return;
    }
    recordClientMetric(metric.scriptError);
  };
  const promiseError = () => recordClientMetric(metric.promiseError);
  const visibility = () => {
    if (document.hidden) {
      flush(state).catch(() => {});
      return;
    }
    if (!state.firstForeground) {
      state.firstForeground = true;
      if (!state.signedIn) state.chatStart = performance.now();
    }
    register();
    for (const element of state.candidates.keys()) {
      state.observer?.unobserve(element);
      state.observer?.observe(element);
    }
  };
  worker?.addEventListener("controllerchange", register);
  worker?.addEventListener("message", dirty);
  document.addEventListener("visibilitychange", visibility);
  window.addEventListener("error", scriptError);
  window.addEventListener("unhandledrejection", promiseError);
  register();
  state.dispose = () => {
    if (tab === state) tab = undefined;
    clearTimeout(state.timer);
    state.observer?.disconnect();
    state.candidates.clear();
    window.removeEventListener("error", scriptError);
    window.removeEventListener("unhandledrejection", promiseError);
    worker?.removeEventListener("controllerchange", register);
    worker?.removeEventListener("message", dirty);
    document.removeEventListener("visibilitychange", visibility);
    safeWorkerMessage(worker?.controller, {
      kind: "commonroom-metrics:stop",
      epoch: state.epoch,
    });
  };
}

export function stopClientMetrics() {
  signIn = undefined;
  tab?.dispose();
}

export function markClientSignIn(base: string) {
  // Timing starts before the authenticated response supplies the sampling policy.
  signIn = { base, started: performance.now() };
}

export function setClientDisplayScope(
  kind: "conversation" | "stickers",
  started?: number,
) {
  const state = tab;
  if (!state) return;
  const target =
    kind === "conversation" ? metric.firstMedia : metric.firstSticker;
  for (const [element, candidate] of state.candidates) {
    if (candidate.metric !== target) continue;
    state.observer?.unobserve(element);
    state.candidates.delete(element);
  }
  if (kind === "conversation") state.conversationStart = started;
  else state.stickerStart = started;
}

export function recordFirstChat(selectedAt?: number): (() => void) | undefined {
  const state = tab;
  if (!state) return;
  const target = state.signedIn ? metric.firstSignedInChat : metric.firstChat;
  if (state.firsts.has(target)) return;
  let frame: number | undefined;
  const afterVisible = () => {
    if (document.hidden || tab !== state || frame !== undefined) return;
    // A layout commit schedules one frame, then one confirmation frame.
    frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => {
        frame = undefined;
        if (tab === state && !document.hidden) {
          recordFirstClientDuration(target, selectedAt ?? state.chatStart);
          document.removeEventListener("visibilitychange", afterVisible);
        }
      });
    });
  };
  document.addEventListener("visibilitychange", afterVisible);
  afterVisible();
  return () => {
    document.removeEventListener("visibilitychange", afterVisible);
    if (frame !== undefined) cancelAnimationFrame(frame);
  };
}

export function observeFirstClientImage(
  element: HTMLImageElement,
  sticker = false,
) {
  const state = tab;
  if (!state || typeof IntersectionObserver !== "function") return;
  const target = sticker ? metric.firstSticker : metric.firstMedia;
  const started = sticker ? state.stickerStart : state.conversationStart;
  if (
    started === undefined ||
    state.firsts.has(target) ||
    state.candidates.has(element)
  )
    return;
  if (state.candidates.size >= 32) {
    for (const candidate of state.candidates.keys()) {
      if (!candidate.isConnected) {
        state.observer?.unobserve(candidate);
        state.candidates.delete(candidate);
      }
    }
    if (state.candidates.size >= 32) return;
  }
  state.observer ??= new IntersectionObserver((entries) => {
    for (const entry of entries) {
      const candidate = state.candidates.get(entry.target);
      if (!candidate) continue;
      if (!entry.target.isConnected) {
        state.observer?.unobserve(entry.target);
        state.candidates.delete(entry.target);
      } else if (
        entry.isIntersecting &&
        entry.intersectionRect.width > 0 &&
        entry.intersectionRect.height > 0 &&
        !document.hidden &&
        tab === state
      ) {
        recordFirstClientDuration(candidate.metric, candidate.started);
        setClientDisplayScope(
          candidate.metric === metric.firstMedia ? "conversation" : "stickers",
        );
      }
    }
  });
  state.candidates.set(element, { metric: target, started });
  state.observer.observe(element);
}

export function recordFirstClientDuration(
  target: ClientMetric,
  started?: number,
) {
  const state = tab;
  if (!state || started === undefined || state.firsts.has(target)) return;
  state.firsts.add(target);
  recordClientDuration(target, started);
}

function safeWorkerMessage(
  controller: ServiceWorker | null | undefined,
  value: object,
) {
  try {
    controller?.postMessage(value);
  } catch {
    // Worker state can change between reading the controller and sending.
  }
}

export function clientMetricClock(): number | undefined {
  return tab ? performance.now() : undefined;
}

export function recordClientMetric(metric: ClientMetric): void {
  const state = tab;
  if (!state) return;
  increment(state, metric, 0, 1, 1);
  schedule(state);
}

export function recordClientDuration(
  metric: ClientMetric,
  started: number | undefined,
): void {
  const state = tab;
  if (!state || started === undefined) return;
  const bucket = durationBucket(performance.now() - started);
  increment(state, metric, bucket, durationBucketCount, 1);
  schedule(state);
}

function increment(
  state: Tab,
  metric: ClientMetric,
  bucket: number,
  length: number,
  count: number,
) {
  if (state.count >= maxBatchCount) return;
  let row = state.rows.get(metric);
  if (!row) {
    row = { buckets: Array<number>(length).fill(0), count: 0 };
    state.rows.set(metric, row);
  }
  const accepted = Math.min(
    count,
    maxMetricCount - row.count,
    maxBatchCount - state.count,
  );
  row.buckets[bucket] = (row.buckets[bucket] ?? 0) + accepted;
  row.count += accepted;
  state.count += accepted;
}

function schedule(state: Tab) {
  state.timer ??= setTimeout(
    () => {
      state.timer = undefined;
      flush(state).catch(() => {});
    },
    Math.max(
      0,
      interval - (performance.now() - lastFlush),
      lastFlush < 0 ? state.firstDelay : 0,
    ),
  );
}

function readWorker(state: Tab): Promise<unknown> {
  const controller = navigator.serviceWorker?.controller;
  if (!controller) return Promise.resolve(null);
  return new Promise((resolve) => {
    const { port1, port2 } = new MessageChannel();
    const finish = (value: unknown) => {
      clearTimeout(timer);
      port1.close();
      resolve(value);
    };
    const timer = setTimeout(() => finish(null), 150);
    port1.onmessage = (event: MessageEvent<unknown>) => finish(event.data);
    try {
      controller.postMessage(
        {
          kind: "commonroom-metrics:read",
          base: state.base,
          epoch: state.epoch,
        },
        [port2],
      );
    } catch {
      finish(null);
    }
  });
}

async function flush(state: Tab) {
  if (tab !== state || (!state.rows.size && !state.workerDirty)) return;
  const now = performance.now();
  if (sending || now - lastFlush < interval) {
    schedule(state);
    return;
  }
  clearTimeout(state.timer);
  state.timer = undefined;
  lastFlush = now;
  sending = true;
  try {
    if (state.workerDirty) {
      state.workerDirty = false;
      const counts = await readWorker(state);
      if (tab !== state) return;
      if (Array.isArray(counts) && counts.length === mediaMetrics.length) {
        for (const [index, metric] of mediaMetrics.entries()) {
          const row: unknown = counts[index];
          const length = mediaBucketCounts[index];
          if (!Array.isArray(row) || row.length !== length) continue;
          for (let bucket = 0; bucket < row.length; bucket++) {
            const count: unknown = row[bucket];
            if (
              typeof count === "number" &&
              Number.isInteger(count) &&
              count > 0 &&
              count <= maxMetricCount
            )
              increment(state, metric, bucket, row.length, count);
          }
        }
      }
    }
    if (state.rows.size) {
      const random = crypto.getRandomValues(new Uint32Array(2));
      const id = (random[0] ?? 0) * 0x100000 + ((random[1] ?? 0) & 0xfffff) + 1;
      const body = JSON.stringify({
        v: 2,
        id,
        c: state.cohort,
        rows: [...state.rows]
          .sort(([left], [right]) => left - right)
          .map(([metric, row]) => [metric, row.buckets]),
      });
      state.rows.clear();
      state.count = 0;
      if (body.length <= 2048) {
        void fetch(state.endpoint, {
          method: "POST",
          credentials: "include",
          keepalive: true,
          redirect: "error",
          priority: "low",
          body,
        }).catch(() => {});
      }
    }
  } catch {
    state.rows.clear();
    state.count = 0;
  } finally {
    if (tab === state) state.registerWorker?.();
    sending = false;
  }
}

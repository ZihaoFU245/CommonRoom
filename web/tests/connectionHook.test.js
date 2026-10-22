import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";
import { api, ApiError } from "../src/api/client.ts";
const registration = mockHooks();
const { useConnection } = await import("../src/hooks/useConnection.ts");
registration.deregister();
const snapshot = {
  kind: "snapshot",
  username: "alice",
  groups: [],
  permissions: [],
  account_access: { id: "alice-id", permissions: [], commands: [] },
  private_access: {},
  private_permissions: [],
  private_commands: [],
  policy_revision: 0,
  admin: false,
  users: [],
  online: [],
  rooms: [],
  direct: [],
  private_peers: [],
  commands: [],
  available_rooms: [],
  unread: {},
};
function fixture(t) {
  const originals = {
    WebSocket: globalThis.WebSocket,
    window: globalThis.window,
    document: globalThis.document,
    location: globalThis.location,
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
  };
  const session = api.session,
    me = api.me;
  const sockets = [],
    refreshes = [],
    snapshots = [],
    checks = [],
    timers = new Map(),
    listeners = new Map();
  let serial = 0,
    loggedOut = 0;
  let refreshError;
  class Socket {
    static OPEN = 1;
    static CLOSED = 3;
    readyState = 0;
    constructor() {
      sockets.push(this);
    }
    close() {
      this.readyState = 3;
      this.onclose?.();
    }
  }
  globalThis.WebSocket = Socket;
  globalThis.window = {
    addEventListener: (name, handler) => listeners.set(name, handler),
    removeEventListener() {},
  };
  globalThis.document = {
    baseURI: "http://localhost/",
    hidden: false,
    addEventListener: (name, handler) => listeners.set(name, handler),
    removeEventListener: (name) => listeners.delete(name),
  };
  globalThis.location = { protocol: "http:" };
  globalThis.setTimeout = (callback, delay) => {
    timers.set(++serial, { callback, delay });
    return serial;
  };
  globalThis.clearTimeout = (id) => timers.delete(id);
  api.session = () =>
    new Promise((resolve, reject) => checks.push({ resolve, reject }));
  api.me = async () => {
    refreshes.push(true);
    if (refreshError) throw refreshError;
    return snapshot;
  };
  const harness = createHookHarness();
  useHarness(harness);
  useConnection(snapshot, {
    onSnapshot(previous, next) {
      snapshots.push(next);
    },
    onResult() {},
    onDisconnect() {},
    onLogout() {
      loggedOut++;
    },
  });
  for (const effect of harness.effects) effect();
  t.after(() => {
    for (const slot of harness.slots) slot?.cleanup?.();
    Object.assign(globalThis, originals);
    api.session = session;
    api.me = me;
  });
  return {
    sockets,
    checks,
    timers,
    listeners,
    refreshes,
    snapshots,
    failRefresh(error) {
      refreshError = error;
    },
    baseline() {
      const ws = sockets.at(-1);
      ws.readyState = Socket.OPEN;
      ws.onopen();
      ws.onmessage({ data: JSON.stringify(snapshot) });
    },
    async runTimers() {
      const pending = [...timers.values()];
      timers.clear();
      for (const timer of pending) timer.callback();
      await new Promise((resolve) => setImmediate(resolve));
    },
    get loggedOut() {
      return loggedOut;
    },
  };
}
test("reconnect backoff starts before a slow session check completes", async (t) => {
  const h = fixture(t);
  h.sockets[0].close();
  assert.equal(h.checks.length, 1);
  assert.equal(h.timers.size, 1);
  const timer = [...h.timers.values()][0];
  assert.equal(timer.delay, 1000);
  timer.callback();
  assert.equal(
    h.sockets.length,
    2,
    "socket reconnect must not wait for /session",
  );
  h.checks[0].reject(new ApiError(401, "old validation"));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    h.loggedOut,
    0,
    "a stale check must not close the replacement socket",
  );
});
test("current session revocation cancels a scheduled reconnect", async (t) => {
  const h = fixture(t);
  h.sockets[0].close();
  h.checks[0].reject(new ApiError(401, "revoked"));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(h.loggedOut, 1);
  assert.equal(h.timers.size, 0);
});
test("returning online retries immediately instead of waiting for backoff", (t) => {
  const h = fixture(t);
  h.sockets[0].close();
  h.listeners.get("online")();
  assert.equal(h.timers.size, 0);
  assert.equal(h.sockets.length, 2);
  h.listeners.get("focus")();
  assert.equal(h.sockets.length, 2, "do not overlap connection attempts");
});

test("resume event bursts share one refresh and hidden pages cancel scheduled work", async (t) => {
  const h = fixture(t);
  h.baseline();
  for (const event of ["focus", "visibilitychange", "pageshow", "online"])
    h.listeners.get(event)();
  assert.equal(h.timers.size, 1);
  assert.equal(h.refreshes.length, 0);
  await h.runTimers();
  assert.equal(h.refreshes.length, 1);
  assert.equal(h.snapshots.length, 2);
  h.listeners.get("focus")();
  document.hidden = true;
  h.listeners.get("visibilitychange")();
  assert.equal(h.timers.size, 0);
  document.hidden = false;
  h.listeners.get("visibilitychange")();
  await h.runTimers();
  assert.equal(
    h.refreshes.length,
    2,
    "a later resume must still repair missed updates",
  );
});

test("reconnect baseline replaces queued resume refreshes without extra HTTP", async (t) => {
  const h = fixture(t);
  const ws = h.sockets[0];
  ws.readyState = WebSocket.OPEN;
  ws.onopen();
  h.listeners.get("focus")();
  assert.equal(
    h.timers.size,
    0,
    "initial socket baseline is already in flight",
  );
  h.baseline();
  h.listeners.get("focus")();
  assert.equal(h.timers.size, 1);
  ws.close();
  assert.equal(h.timers.size, 1, "only reconnect backoff remains");
  h.listeners.get("online")();
  h.baseline();
  await h.runTimers();
  assert.equal(h.refreshes.length, 0);
  assert.equal(h.snapshots.length, 2);
});

test("debounced refresh errors still detect revoked sessions", async (t) => {
  const h = fixture(t);
  h.baseline();
  h.failRefresh(new ApiError(401, "revoked"));
  h.listeners.get("focus")();
  await h.runTimers();
  assert.equal(h.loggedOut, 1);
  assert.equal(h.timers.size, 0);
});

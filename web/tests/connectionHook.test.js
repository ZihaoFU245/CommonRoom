import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
import { api, ApiError } from "../src/api/client.ts";
let effects = [];
globalThis.connectionTestHooks = {
  state(value) {
    return [typeof value === "function" ? value() : value, () => {}];
  },
  ref(value) {
    return { current: value };
  },
  effect(callback) {
    effects.push(callback);
  },
};
const registration = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "preact/hooks"
      ? { url: "test:connection-hooks", shortCircuit: true }
      : next(specifier, context);
  },
  load(url, context, next) {
    return url === "test:connection-hooks"
      ? {
          format: "module",
          shortCircuit: true,
          source:
            "export const useState=(...a)=>globalThis.connectionTestHooks.state(...a); export const useRef=(...a)=>globalThis.connectionTestHooks.ref(...a); export const useEffect=(...a)=>globalThis.connectionTestHooks.effect(...a);",
        }
      : next(url, context);
  },
});
const { useConnection } = await import("../src/hooks/useConnection.ts");
registration.deregister();
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
    checks = [],
    timers = new Map(),
    listeners = new Map();
  let serial = 0,
    loggedOut = 0;
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
    addEventListener() {},
    removeEventListener() {},
  };
  globalThis.location = { protocol: "http:" };
  globalThis.setTimeout = (callback, delay) => {
    timers.set(++serial, { callback, delay });
    return serial;
  };
  globalThis.clearTimeout = (id) => timers.delete(id);
  api.session = () =>
    new Promise((resolve, reject) => checks.push({ resolve, reject }));
  api.me = async () => ({});
  effects = [];
  useConnection(
    {},
    {
      onSnapshot() {},
      onResult() {},
      onDisconnect() {},
      onLogout() {
        loggedOut++;
      },
    },
  );
  const cleanups = effects.map((effect) => effect());
  t.after(() => {
    for (const cleanup of cleanups) cleanup?.();
    Object.assign(globalThis, originals);
    api.session = session;
    api.me = me;
  });
  return {
    sockets,
    checks,
    timers,
    listeners,
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

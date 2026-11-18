import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";
import { api, ApiError } from "../src/api/client.ts";
import { mediaStatusCache } from "../src/features/conversation/mediaStatusCache.ts";
import { createRequestLifecycle } from "../src/features/conversation/requestLifecycle.ts";
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
function fixture(t, initial = snapshot) {
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
    mediaUpdates = [],
    timers = new Map(),
    listeners = new Map();
  let serial = 0,
    loggedOut = 0,
    disconnected = 0;
  let refreshError;
  class Socket {
    static OPEN = 1;
    static CLOSED = 3;
    readyState = 0;
    constructor(url) {
      this.url = url;
      sockets.push(this);
    }
    close() {
      this.readyState = 3;
      this.onclose?.();
    }
  }
  globalThis.WebSocket = Socket;
  globalThis.window = {
    dispatchEvent(event) {
      mediaUpdates.push(event.type);
      listeners.get(event.type)?.(event);
      return true;
    },
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
  const results = [],
    frames = [];
  const lifecycle = createRequestLifecycle(
    (frame) => frames.push(frame),
    () => {},
  );
  function render(nameColors = false) {
    useHarness(harness);
    harness.effects = [];
    const result = useConnection(
      initial,
      {
        onSnapshot(previous, next) {
          snapshots.push(next);
        },
        onResult: lifecycle.receive,
        onDisconnect() {
          disconnected++;
          lifecycle.disconnect();
        },
        whenIdle: lifecycle.whenIdle,
        onLogout() {
          loggedOut++;
        },
      },
      nameColors,
    );
    for (const effect of harness.effects) effect();
    return result;
  }
  render();
  t.after(() => {
    for (const slot of harness.slots) slot?.cleanup?.();
    Object.assign(globalThis, originals);
    api.session = session;
    api.me = me;
  });
  return {
    sockets,
    results,
    frames,
    render,
    submit(parallel = false) {
      return lifecycle.submit({
        command: {
          text: parallel ? "message" : "/help",
          room: "lobby",
          key: null,
        },
        frame: { text: "hello", room: "lobby" },
        parallel,
        complete: (result) => results.push(result),
      });
    },
    get disconnected() {
      return disconnected;
    },
    checks,
    timers,
    listeners,
    refreshes,
    snapshots,
    mediaUpdates,
    failRefresh(error) {
      refreshError = error;
    },
    baseline() {
      const ws = sockets.at(-1);
      ws.readyState = Socket.OPEN;
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
test("checkpoint is offered on reconnect and waits for the resume marker", (t) => {
  const token = "b3b2a710-eed4-4715-94e4-735566ec8326";
  const h = fixture(t, { ...snapshot, resume: token });
  const socket = h.sockets[0];
  assert.equal(socket.url.searchParams.get("resume"), token);
  socket.readyState = WebSocket.OPEN;
  socket.onmessage({
    data: JSON.stringify({ kind: "presence", online: ["alice"] }),
  });
  assert.equal(h.render().status, "connecting");
  assert.equal(h.render().socket.current, null);
  socket.onmessage({ data: JSON.stringify({ kind: "resumed" }) });
  assert.equal(h.render().status, "online");
  assert.equal(h.render().socket.current, socket);
  assert.deepEqual(h.render().state.online, ["alice"]);
  socket.close();
  h.listeners.get("online")();
  assert.equal(h.sockets[1].url.searchParams.get("resume"), token);
});

test("a delta with a missing base reconnects without a checkpoint", (t) => {
  const h = fixture(t, {
    ...snapshot,
    resume: "b3b2a710-eed4-4715-94e4-735566ec8326",
  });
  const socket = h.sockets[0];
  assert.equal(socket.url.searchParams.get("delta"), "1");
  socket.readyState = WebSocket.OPEN;
  socket.onmessage({
    data: JSON.stringify({
      kind: "conversation_delta",
      view: "lobby",
      id: "unknown",
      base_revision: 1,
      sequences: [],
      upsert: [],
      unread: { revision: 2, count: 0, first: null, oldest: 0, through: 0 },
    }),
  });
  assert.equal(h.render().status, "offline");
  h.listeners.get("online")();
  assert.equal(h.sockets[1].url.searchParams.has("resume"), false);
});

test("media invalidations refresh viewers without changing conversation state", async (t) => {
  const h = fixture(t);
  t.after(() => mediaStatusCache.clear());
  h.baseline();
  await mediaStatusCache.load("library-item", async () => ({ saved: true }));
  const before = h.snapshots.length;
  h.sockets.at(-1).onmessage({ data: '{"kind":"media"}' });
  assert.equal(mediaStatusCache.peek("library-item"), undefined);
  assert.deepEqual(h.mediaUpdates, ["commonroom-library-change"]);
  assert.equal(h.snapshots.length, before);
  assert.equal(h.refreshes.length, 0);
});
test("snapshots invalidate media only after authority changes", async (t) => {
  const h = fixture(t);
  h.baseline();
  await mediaStatusCache.load("unchanged", async () => ({
    saved: true,
    expires: null,
    expired: false,
  }));
  const ws = h.sockets.at(-1);
  ws.onmessage({
    data: JSON.stringify({ ...snapshot, preferences: { theme: "dark" } }),
  });
  assert.ok(mediaStatusCache.peek("unchanged"));
  assert.equal(h.mediaUpdates.length, 0);
  ws.onmessage({ data: JSON.stringify({ ...snapshot, policy_revision: 1 }) });
  assert.equal(mediaStatusCache.peek("unchanged"), undefined);
  assert.equal(h.mediaUpdates.length, 1);
});
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

for (const parallel of [false, true]) {
  test(`changing color subscription drains ${parallel ? "parallel messages" : "commands"} before reconnecting`, (t) => {
    const h = fixture(t);
    h.baseline();
    h.submit(parallel);
    if (parallel) h.submit(true);
    const old = h.sockets[0];
    h.render(true);
    assert.equal(h.sockets.length, 1);
    assert.equal(old.readyState, WebSocket.OPEN);
    for (const frame of h.frames) {
      old.onmessage({
        data: JSON.stringify({ kind: "notice", id: frame.id, text: "sent" }),
      });
    }
    assert.equal(h.results.length, h.frames.length);
    assert.ok(h.results.every((result) => result.kind === "notice"));
    assert.equal(h.disconnected, 0);
    assert.equal(old.readyState, WebSocket.CLOSED);
    assert.equal(h.sockets.length, 2);
    assert.equal(new URL(h.sockets[1].url).searchParams.get("colors"), "1");
    h.baseline();
    h.render(false);
    assert.equal(h.sockets.length, 3);
    assert.equal(new URL(h.sockets[2].url).searchParams.has("colors"), false);
  });
}

test("reverting a queued preference cancels reconnect and real disconnect still fails pending requests", (t) => {
  const h = fixture(t);
  h.baseline();
  h.submit();
  h.render(true);
  h.render(false);
  h.sockets[0].onmessage({
    data: JSON.stringify({ kind: "notice", id: h.frames[0].id, text: "sent" }),
  });
  assert.equal(h.sockets.length, 1);
  h.submit(true);
  h.render(true);
  h.sockets[0].close();
  assert.equal(h.results[1].kind, "error");
  assert.match(h.results[1].text, /Connection lost/);
  assert.equal(h.disconnected, 1);
  assert.equal(h.sockets.length, 2);
  assert.equal(
    h.timers.size,
    0,
    "idle reconnect must not also schedule backoff",
  );
});

test("streaming bot drafts keep each bot in place", (t) => {
  const h = fixture(t);
  h.baseline();
  const ws = h.sockets.at(-1);
  const expires = Math.floor(Date.now() / 1000) + 30;
  const working = (account, draft) => ({
    kind: "activity",
    conversation_id: "lobby",
    account_id: account,
    username: account,
    state: "working",
    expires,
    ...(draft === undefined ? {} : { draft }),
  });
  ws.onmessage({ data: JSON.stringify(working("alpha")) });
  ws.onmessage({ data: JSON.stringify(working("beta")) });
  ws.onmessage({ data: JSON.stringify(working("alpha", "answering")) });
  const first = h.render();
  assert.deepEqual(
    first.botActivities.map((item) => item.account_id),
    ["alpha", "beta"],
    "a streaming draft replaces its row instead of moving it",
  );
  assert.equal(first.botActivities[0].draft, "answering");
  ws.onmessage({
    data: JSON.stringify({ ...working("alpha"), state: "idle" }),
  });
  const second = h.render();
  assert.deepEqual(
    second.botActivities.map((item) => item.account_id),
    ["beta"],
  );
});

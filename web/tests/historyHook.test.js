import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
import { api } from "../src/api/client.ts";

// Run the real hook with deterministic state/effect scheduling and mocked HTTP.
let active;
const registration = registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "preact/hooks")
      return { url: "test:history-hooks", shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "test:history-hooks")
      return {
        format: "module",
        shortCircuit: true,
        source:
          "export const useState = (...a) => globalThis.historyTestHooks.state(...a); export const useRef = (...a) => globalThis.historyTestHooks.ref(...a); export const useEffect = (...a) => globalThis.historyTestHooks.effect(...a); export const useMemo = (...a) => globalThis.historyTestHooks.memo(...a);",
      };
    return next(url, context);
  },
});
globalThis.historyTestHooks = {
  memo(callback, deps) {
    const index = active.index++;
    const previous = active.slots[index];
    if (
      !previous ||
      deps.some((value, i) => !Object.is(value, previous.deps[i]))
    )
      active.slots[index] = { deps, value: callback() };
    return active.slots[index].value;
  },
  state(initial) {
    const index = active.index++;
    if (!(index in active.slots)) active.slots[index] = initial;
    const owner = active;
    return [
      owner.slots[index],
      (update) => {
        const value =
          typeof update === "function" ? update(owner.slots[index]) : update;
        if (!Object.is(value, owner.slots[index])) {
          owner.slots[index] = value;
          owner.dirty = true;
        }
      },
    ];
  },
  ref(initial) {
    const index = active.index++;
    return (active.slots[index] ??= { current: initial });
  },
  effect(callback, deps) {
    const index = active.index++;
    const previous = active.slots[index];
    if (
      !previous ||
      deps.some((value, i) => !Object.is(value, previous.deps[i]))
    ) {
      active.effects.push(() => {
        previous?.cleanup?.();
        active.slots[index] = { deps, cleanup: callback() };
      });
    }
  },
};
const { useHistory } = await import("../src/hooks/useHistory.ts");
registration.deregister();

test("completed jumps allow live merging and refresh around the current anchor", async () => {
  const original = api.history;
  const requests = [];
  const message = (sequence) => ({
    id: String(sequence),
    sequence,
    time: sequence,
    from: "alice",
    to: null,
    text: "hello",
    reactions: {},
    reply: null,
    mentions: [],
  });
  const state = {
    rooms: [{ name: "team", id: "stable", messages: [] }],
    direct: [],
    private_access: {},
    unread: { team: { revision: 1, through: 200, oldest: 1 } },
    policy_revision: 0,
  };
  const snapshot = { current: state };
  const ledger = { current: { sequence: 0, messages: new Map() } };
  const anchor = { current: { anchor: () => 90 } };
  const bottom = { current: true };
  const harness = { slots: [], index: 0, effects: [], dirty: true };
  let tail = [message(200)];
  let result;
  api.history = async (view, signal, range) => {
    requests.push(range);
    return {
      view,
      revision: state.unread.team.revision,
      messages: range.around ? [message(range.around)] : tail,
      has_before: true,
      has_after: !!range.around,
    };
  };
  async function settle() {
    for (let round = 0; round < 20; round++) {
      if (harness.dirty) {
        active = harness;
        harness.index = 0;
        harness.effects = [];
        harness.dirty = false;
        result = useHistory(
          state,
          "team",
          tail,
          snapshot,
          ledger,
          anchor,
          bottom,
        );
        for (const effect of harness.effects) effect();
      }
      await new Promise((resolve) => setImmediate(resolve));
      if (!harness.dirty) return;
    }
    assert.fail("hook did not settle");
  }
  try {
    await settle();
    result.loadSequence(50);
    await settle();
    assert.deepEqual(requests.at(-1), { around: 50 });
    bottom.current = false;
    state.unread.team = { ...state.unread.team, revision: 2 };
    harness.dirty = true;
    await settle();
    assert.deepEqual(
      requests.at(-1),
      { around: 90 },
      JSON.stringify({ target: harness.slots[5], result, requests }),
    );
    // A revision represented by one new live message uses the merge shortcut.
    const count = requests.length;
    state.unread.team = { ...state.unread.team, revision: 3, through: 201 };
    tail = [message(201)];
    harness.dirty = true;
    await settle();
    assert.equal(requests.length, count);
    assert.equal(result.historyReady, true);
  } finally {
    api.history = original;
    for (const slot of harness.slots) slot?.cleanup?.();
  }
});

test("busy history preserves explicit paging, bounds retries and keeps live tails visible", async () => {
  const original = api.history;
  const message = (sequence) => ({
    id: String(sequence),
    sequence,
    time: sequence,
    from: "alice",
    to: null,
    text: "hello",
    reactions: {},
    reply: null,
    mentions: [],
  });
  const state = {
    rooms: [{ name: "team", id: "stable", messages: [message(200)] }],
    direct: [],
    private_access: {},
    unread: { team: { revision: 1, through: 200, oldest: 1 } },
    policy_revision: 0,
  };
  const snapshot = { current: state };
  const ledger = { current: { sequence: 0, messages: new Map() } };
  const harness = { slots: [], index: 0, effects: [], dirty: true };
  let result;
  const requests = [];
  let busy = false;
  api.history = async (view, signal, range) => {
    requests.push(range);
    const revision = state.unread.team.revision;
    if (busy)
      state.unread.team = { ...state.unread.team, revision: revision + 1 };
    return {
      view,
      revision,
      messages: range.before ? [message(100)] : [message(200)],
      has_before: true,
      has_after: false,
    };
  };
  async function settle() {
    for (let round = 0; round < 20; round++) {
      if (harness.dirty) {
        active = harness;
        harness.index = 0;
        harness.effects = [];
        harness.dirty = false;
        result = useHistory(
          state,
          "team",
          state.rooms[0].messages,
          snapshot,
          ledger,
          { current: { anchor: () => 200 } },
          { current: true },
        );
        for (const effect of harness.effects) effect();
      }
      await new Promise((resolve) => setImmediate(resolve));
      if (!harness.dirty) return;
    }
    assert.fail("busy history did not settle");
  }
  try {
    await settle();
    busy = true;
    result.loadOlder();
    await settle();
    assert.deepEqual(requests.slice(1, 4), [
      { before: 200 },
      { before: 200 },
      { before: 200 },
    ]);
    assert.ok(result.messages.some((m) => m.sequence === 100));
    assert.equal(result.historyReady, true);
    assert.match(result.historyError, /changing quickly/);
    // A broken history endpoint cannot hide newly received live messages.
    api.history = async () => {
      throw new Error("unavailable");
    };
    result.retryHistory();
    await settle();
    state.rooms = [
      {
        ...state.rooms[0],
        messages: [...state.rooms[0].messages, message(201)],
      },
    ];
    state.unread.team = { ...state.unread.team, revision: 100 };
    harness.dirty = true;
    await settle();
    assert.ok(ledger.current.messages.get("201") > 0);
    assert.ok(result.messages.some((message) => message.sequence === 201));
    assert.match(result.historyError, /unavailable/);
  } finally {
    api.history = original;
    for (const slot of harness.slots) slot?.cleanup?.();
  }
});

test("returning to a cached conversation paints history before a delayed refresh", async () => {
  const original = api.history;
  const state = {
    rooms: [
      { name: "a", id: "a-id", messages: [] },
      { name: "b", id: "b-id", messages: [] },
    ],
    direct: [],
    private_access: {},
    unread: {
      a: { revision: 1, through: 10, oldest: 1 },
      b: { revision: 1, through: 20, oldest: 1 },
    },
    policy_revision: 0,
  };
  const snapshot = { current: state },
    ledger = { current: { sequence: 0, messages: new Map() } },
    timeline = { current: { anchor: () => 5 } },
    bottom = { current: false };
  const harness = { slots: [], index: 0, effects: [], dirty: true };
  let selected = "a",
    result,
    resolve;
  api.history = async (view) => ({
    view,
    revision: 1,
    messages: [
      {
        id: view,
        sequence: view === "a" ? 5 : 20,
        text: view,
        from: "alice",
        time: 1,
        mentions: [],
        reactions: {},
        reply: null,
      },
    ],
    has_before: true,
    has_after: true,
  });
  function render(flush = true) {
    active = harness;
    harness.index = 0;
    harness.effects = [];
    harness.dirty = false;
    result = useHistory(
      state,
      selected,
      [],
      snapshot,
      ledger,
      timeline,
      bottom,
    );
    if (flush) for (const effect of harness.effects) effect();
  }
  async function settle() {
    for (let i = 0; i < 10; i++) {
      if (harness.dirty) render();
      await new Promise((r) => setImmediate(r));
      if (!harness.dirty) return;
    }
    assert.fail("did not settle");
  }
  try {
    await settle();
    selected = "b";
    harness.dirty = true;
    await settle();
    state.unread.a = { ...state.unread.a, revision: 2 };
    api.history = (view) =>
      new Promise((done) => {
        resolve = () =>
          done({
            view,
            revision: 2,
            messages: [
              {
                id: "updated",
                sequence: 5,
                text: "edited",
                from: "alice",
                time: 1,
                mentions: [],
                reactions: {},
                reply: null,
              },
            ],
            has_before: true,
            has_after: true,
          });
      });
    selected = "a";
    render(false);
    assert.equal(
      result.messages[0].text,
      "a",
      "cache is visible before effects run",
    );
    for (const effect of harness.effects) effect();
    await settle();
    assert.equal(result.messages[0].text, "a");
    assert.equal(result.refreshing, true);
    resolve();
    await settle();
    assert.equal(result.messages[0].text, "edited");
    state.rooms = state.rooms.filter((room) => room.id !== "a-id");
    delete state.unread.a;
    harness.dirty = true;
    await settle();
    assert.deepEqual(result.messages, []);
    assert.equal(result.historyReady, false);
  } finally {
    api.history = original;
    for (const slot of harness.slots) slot?.cleanup?.();
  }
});

test("a complete snapshot tail renders immediately without a redundant HTTP page", () => {
  const original = api.history;
  api.history = () => {
    assert.fail("complete snapshot must not fetch history");
  };
  const tail = [
    {
      id: "m1",
      sequence: 10,
      text: "hello",
      from: "alice",
      time: 1,
      mentions: [],
      reactions: {},
      reply: null,
    },
  ];
  const state = {
    rooms: [{ name: "team", id: "stable", messages: tail }],
    direct: [],
    private_access: {},
    unread: { team: { revision: 1, through: 10, oldest: 10 } },
    policy_revision: 0,
  };
  const harness = { slots: [], index: 0, effects: [], dirty: true };
  try {
    active = harness;
    const result = useHistory(
      state,
      "team",
      tail,
      { current: state },
      { current: { sequence: 0, messages: new Map() } },
      { current: null },
      { current: true },
    );
    assert.equal(result.historyReady, true);
    assert.equal(result.messages[0].text, "hello");
    for (const effect of harness.effects) effect();
  } finally {
    api.history = original;
    for (const slot of harness.slots) slot?.cleanup?.();
  }
});

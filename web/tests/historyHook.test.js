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
          "export const useState = (...a) => globalThis.historyTestHooks.state(...a); export const useRef = (...a) => globalThis.historyTestHooks.ref(...a); export const useEffect = (...a) => globalThis.historyTestHooks.effect(...a);",
      };
    return next(url, context);
  },
});
globalThis.historyTestHooks = {
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
    delete globalThis.historyTestHooks;
  }
});

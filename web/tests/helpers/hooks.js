import { registerHooks } from "node:module";

// Deterministic hook scheduling for state/race tests. Browser fixtures cover real
// Preact commit ordering and DOM effects; this helper deliberately has no DOM.
let active;
const changed = (previous, deps) =>
  !previous ||
  !deps ||
  deps.some((value, i) => !Object.is(value, previous.deps?.[i]));
export function createHookHarness() {
  return { slots: [], index: 0, effects: [], dirty: true };
}
export function useHarness(harness) {
  active = harness;
  harness.index = 0;
}
const hooks = {
  ref(initial) {
    return (active.slots[active.index++] ??= { current: initial });
  },
  state(initial) {
    const owner = active;
    const index = owner.index++;
    if (!(index in owner.slots))
      owner.slots[index] = typeof initial === "function" ? initial() : initial;
    return [
      owner.slots[index],
      (update) => {
        const next =
          typeof update === "function" ? update(owner.slots[index]) : update;
        if (!Object.is(next, owner.slots[index])) {
          owner.slots[index] = next;
          owner.dirty = true;
        }
      },
    ];
  },
  memo(callback, deps) {
    const index = active.index++;
    if (changed(active.slots[index], deps))
      active.slots[index] = { deps, value: callback() };
    return active.slots[index].value;
  },
  effect(callback, deps) {
    const owner = active;
    const index = owner.index++;
    const previous = owner.slots[index];
    if (changed(previous, deps))
      owner.effects.push(() => {
        previous?.cleanup?.();
        owner.slots[index] = { deps, cleanup: callback() };
      });
  },
};
globalThis.__chatroomTestHooks = hooks;
export function mockHooks() {
  return registerHooks({
    resolve(specifier, context, next) {
      return specifier === "preact/hooks"
        ? { url: "test:chatroom-hooks", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "test:chatroom-hooks"
        ? {
            format: "module",
            shortCircuit: true,
            source: `
            const hooks = globalThis.__chatroomTestHooks;
            export const useRef = (...args) => hooks.ref(...args);
            export const useState = (...args) => hooks.state(...args);
            export const useMemo = (...args) => hooks.memo(...args);
            export const useCallback = (fn, deps) => hooks.memo(() => fn, deps);
            export const useEffect = (...args) => hooks.effect(...args);
            export const useLayoutEffect = useEffect;
          `,
          }
        : next(url, context);
    },
  });
}

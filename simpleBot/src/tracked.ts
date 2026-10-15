import type { State } from "./state.ts";

/** Track nested mutations without serializing the entire worker state per send.
 * Callers must retrieve inserted objects through state before mutating them. */
export function trackState(
  state: State,
  changed: (key: string) => void,
): State {
  const proxies = new WeakMap<object, object>();
  const wrap = <T extends object>(value: T, mark: () => void): T => {
    const cached = proxies.get(value);
    if (cached) return cached as T;
    const proxy = new Proxy(value, {
      get(target, key, receiver) {
        const item: unknown = Reflect.get(target, key, receiver);
        return typeof item === "object" && item !== null
          ? wrap(item, mark)
          : item;
      },
      set(target, key, item) {
        if (Reflect.get(target, key) !== item) mark();
        return Reflect.set(target, key, item);
      },
      deleteProperty(target, key) {
        if (Reflect.has(target, key)) mark();
        return Reflect.deleteProperty(target, key);
      },
    });
    proxies.set(value, proxy);
    proxies.set(proxy, proxy);
    return proxy;
  };
  for (const field of [
    "cursors",
    "revisions",
    "memory",
    "tasks",
    "parts",
  ] as const) {
    Reflect.set(
      state,
      field,
      new Proxy(state[field], {
        get(target, id, receiver) {
          const value: unknown = Reflect.get(target, id, receiver);
          return typeof value === "object" && value !== null
            ? wrap(value, () => changed(JSON.stringify([field, id])))
            : value;
        },
        set(target, id, value) {
          changed(JSON.stringify([field, id]));
          return Reflect.set(target, id, value);
        },
        deleteProperty(target, id) {
          changed(JSON.stringify([field, id]));
          return Reflect.deleteProperty(target, id);
        },
      }),
    );
  }
  return new Proxy(state, {
    set(target, key, value) {
      if (key !== "account" && key !== "version")
        throw new Error("Worker state collections cannot be replaced");
      if (Reflect.get(target, key) !== value)
        changed(JSON.stringify(["meta", key]));
      return Reflect.set(target, key, value);
    },
  });
}

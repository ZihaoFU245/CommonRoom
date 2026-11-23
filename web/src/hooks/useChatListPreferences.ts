import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "preact/hooks";
import { readStorageJson, writeStorage } from "../api/browserStorage.ts";
import type { Snapshot } from "../api/protocol.ts";
import { serverBaseUrl } from "../api/server.ts";
import { useEvent } from "./useEvent.ts";

type Flag = "pinned" | "unread";
type Preferences = Record<Flag, string[]>;

function load(key: string): Preferences {
  const data = readStorageJson(key);
  const list = (field: Flag): string[] => {
    const value: unknown =
      data && typeof data === "object" && Reflect.get(data, field);
    return Array.isArray(value)
      ? value.filter((item) => typeof item === "string")
      : [];
  };
  return { pinned: list("pinned"), unread: list("unread") };
}

export function useChatListPreferences(state: Snapshot) {
  const key = `commonroom-chat-list:${JSON.stringify([serverBaseUrl(), state.account_access.id])}`;
  const [stored, setStored] = useState(() => ({ key, value: load(key) }));
  const volatile = useRef<typeof stored>();
  const value = stored.key === key ? stored.value : load(key);
  const ids = useMemo(
    () =>
      new Map([
        ...state.rooms.map(({ name, id }) => [name, id] as const),
        ...Object.entries(state.private_access).map(
          ([view, { id }]) => [view, id] as const,
        ),
      ]),
    [state.rooms, state.private_access],
  );
  const identity = useCallback(
    (view: string) => {
      const id = ids.get(view);
      return id ? `id:${id}` : view;
    },
    [ids],
  );
  const change = useEvent((flag: Flag, view: string, enable: boolean) => {
    const current =
      volatile.current?.key === key ? volatile.current.value : load(key);
    const id = identity(view);
    const items = current[flag];
    if (enable === (items.includes(id) || items.includes(view))) return;
    const next = {
      key,
      value: {
        ...current,
        [flag]: enable
          ? [...items, id]
          : items.filter((item) => item !== id && item !== view),
      },
    };
    volatile.current = writeStorage(key, JSON.stringify(next.value))
      ? undefined
      : next;
    setStored(next);
  });
  const clearUnread = useEvent((view: string) => change("unread", view, false));
  useEffect(() => {
    const sync = (event: StorageEvent) => {
      if (event.key !== key && event.key !== null) return;
      volatile.current = undefined;
      setStored({ key, value: load(key) });
    };
    window.addEventListener("storage", sync);
    return () => window.removeEventListener("storage", sync);
  }, [key]);
  return useMemo(() => {
    const pins = new Map(value.pinned.map((id, index) => [id, index]));
    const unread = new Set(value.unread);
    return {
      pinPosition: (view: string) => pins.get(identity(view)) ?? pins.get(view),
      isMarkedUnread: (view: string) =>
        unread.has(identity(view)) || unread.has(view),
      change,
      clearUnread,
    };
  }, [value, identity, change, clearUnread]);
}

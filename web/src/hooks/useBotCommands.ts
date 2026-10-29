import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "preact/hooks";
import { api } from "../api/client.ts";
import type { BotCommands } from "../api/protocol.ts";
import {
  BotCommandCache,
  BOT_COMMAND_REFRESH_MS,
} from "../features/conversation/botCommandCache.ts";
const EMPTY: BotCommands[] = [];

/** Fetch one directory in the background; typing and conversation switching are local. */
export function useBotCommands(
  conversationId: string | undefined,
  versions: ReadonlyMap<string, number>,
  authority: string,
  conversations: ReadonlyMap<string, string>,
  online: boolean,
) {
  const [, render] = useState(0);
  const cache = useMemo(
    () =>
      new BotCommandCache(api.botCommands, () => render((value) => value + 1)),
    // A changed account, server instance or policy must never reuse private metadata.
    [authority],
  );
  const latest = useRef({ conversations, versions });
  latest.current = { conversations, versions };
  const pending = useRef<number | undefined>(undefined);
  const refresh = useCallback(() => {
    if (!document.hidden)
      cache.refresh(latest.current.conversations, latest.current.versions);
  }, [cache]);
  useEffect(() => () => cache.stop(), [cache]);
  useEffect(() => {
    if (!online) return;
    const retry = window.setInterval(refresh, BOT_COMMAND_REFRESH_MS);
    const focus = refresh;
    window.addEventListener("focus", focus);
    return () => {
      window.clearTimeout(pending.current);
      pending.current = undefined;
      window.clearInterval(retry);
      window.removeEventListener("focus", focus);
    };
  }, [cache, online, refresh]);
  useEffect(() => {
    if (!online || pending.current !== undefined) return;
    // Coalesce a burst without moving its first scheduled deadline.
    pending.current = window.setTimeout(() => {
      pending.current = undefined;
      refresh();
    }, 100);
  }, [refresh, versions, conversations, online]);
  const members = conversationId
    ? conversations.get(conversationId)
    : undefined;
  return conversationId && members !== undefined
    ? (cache.get(conversationId, members) ?? EMPTY)
    : EMPTY;
}

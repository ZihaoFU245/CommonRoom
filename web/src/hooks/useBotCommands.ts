import { useEffect, useMemo, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import type { BotCommands } from "../api/protocol.ts";
import { BotCommandCache } from "../features/conversation/botCommandCache.ts";
const EMPTY: BotCommands[] = [];

/** Warm nearby catalogues and refresh in the background; typing is always local. */
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
  useEffect(() => () => cache.stop(), [cache]);
  useEffect(() => {
    cache.retain(new Set(conversations.keys()));
    if (!online) return;
    const refresh = (force: boolean) => {
      const ids = [
        ...new Set([
          ...(conversationId ? [conversationId] : []),
          ...conversations.keys(),
        ]),
      ].slice(0, 32);
      for (const id of ids) {
        const members = conversations.get(id);
        if (members !== undefined)
          cache.refresh(
            id,
            members,
            versions.get(id) ?? 0,
            force,
            id === conversationId,
          );
      }
    };
    refresh(false);
    const focus = () => refresh(true);
    window.addEventListener("focus", focus);
    return () => window.removeEventListener("focus", focus);
  }, [cache, conversationId, versions, conversations, online]);
  const members = conversationId
    ? conversations.get(conversationId)
    : undefined;
  return conversationId && members !== undefined
    ? (cache.get(conversationId, members) ?? EMPTY)
    : EMPTY;
}

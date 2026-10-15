import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import type { BotCommands } from "../api/protocol.ts";
const EMPTY: BotCommands[] = [];

/** Read the server's cached metadata on selection/invalidation, never on typing. */
export function useBotCommands(
  conversationId: string | undefined,
  revision: number,
  policyRevision: number,
  members: string,
  online: boolean,
) {
  const [directory, setDirectory] = useState<{
    id: string;
    bots: BotCommands[];
  } | null>(null);
  useEffect(() => {
    setDirectory(null);
    if (!conversationId || !online) return;
    let stopped = false;
    let request: AbortController | undefined;
    const refresh = () => {
      request?.abort();
      const controller = new AbortController();
      request = controller;
      api
        .botCommands(conversationId, controller.signal)
        .then((result) => {
          if (
            !stopped &&
            !controller.signal.aborted &&
            result.conversation_id === conversationId
          )
            setDirectory({ id: conversationId, bots: result.bots });
        })
        .catch(() => {
          if (!stopped && !controller.signal.aborted) setDirectory(null);
        });
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => {
      stopped = true;
      request?.abort();
      window.removeEventListener("focus", refresh);
    };
  }, [conversationId, revision, policyRevision, members, online]);
  return online && directory !== null && directory.id === conversationId
    ? directory.bots
    : EMPTY;
}

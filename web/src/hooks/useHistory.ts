import type { RefObject } from "preact";
import { useEffect, useMemo, useState } from "preact/hooks";
import type { History, Message, Snapshot, Ledger } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { retainedHistory } from "../features/conversation/unread.ts";
import { ingest } from "../features/conversation/console.ts";
import type { ValueRef } from "./refs.ts";
export function useHistory(
  state: Snapshot,
  selected: string,
  tail: Message[],
  historyCache: RefObject<History | null>,
  currentState: ValueRef<Snapshot>,
  selectedRef: RefObject<string>,
  ledger: ValueRef<Ledger>,
) {
  const [loadedHistory, setLoadedHistory] = useState<History | null>(null);
  const [historyError, setHistoryError] = useState("");
  const [historyRetry, setHistoryRetry] = useState(0);
  const unread = state.unread?.[selected];
  const historyReady =
    !!unread &&
    loadedHistory?.view === selected &&
    loadedHistory.revision === unread.revision;
  const messages = useMemo(
    () =>
      loadedHistory?.view === selected && unread
        ? retainedHistory(loadedHistory.messages, tail, unread)
        : tail,
    [loadedHistory, tail, selected, unread?.oldest, unread?.through],
  );
  // Fetch one conversation on demand, rather than sending every retained
  // message to every connection. Read acknowledgements leave revision alone.
  useEffect(() => {
    setHistoryError("");
    if (!unread || historyReady) return;
    const cache = historyCache.current;
    if (cache?.view === selected) {
      const last = cache.messages.at(-1)?.sequence || 0;
      const additions = tail.filter(
        (message) => message.sequence > last,
      ).length;
      if (additions && cache.revision + additions === unread.revision) {
        const data = {
          view: selected,
          revision: unread.revision,
          messages: retainedHistory(cache.messages, tail, unread),
        };
        historyCache.current = data;
        setLoadedHistory(data);
        return;
      }
    }
    const controller = new AbortController();
    api
      .history(selected, controller.signal)
      .then((data) => {
        if (
          controller.signal.aborted ||
          selectedRef.current !== data.view ||
          currentState.current.unread?.[data.view]?.revision !== data.revision
        )
          return;
        historyCache.current = data;
        ingest(currentState.current, ledger.current, data.messages);
        setLoadedHistory(data);
      })
      .catch((error) => {
        if (!controller.signal.aborted) setHistoryError(errorMessage(error));
      });
    return () => controller.abort();
  }, [selected, unread?.revision, historyRetry]);
  return {
    unread,
    historyReady,
    messages,
    historyError,
    retryHistory: () => setHistoryRetry((value) => value + 1),
  };
}

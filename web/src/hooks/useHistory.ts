import type { RefObject } from "preact";
import type { VirtualTimelineHandle } from "./useVirtualTimeline.ts";
import { useEffect, useRef, useState } from "preact/hooks";
import type {
  HistoryRange,
  Message,
  Snapshot,
  Ledger,
} from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { ingest } from "../features/conversation/console.ts";
import {
  mergePage,
  storeHistory,
} from "../features/conversation/historyCache.ts";
import type { CachedHistory } from "../features/conversation/historyCache.ts";
import type { ValueRef } from "./refs.ts";

export function useHistory(
  state: Snapshot,
  selected: string,
  tail: Message[],
  currentState: ValueRef<Snapshot>,
  ledger: ValueRef<Ledger>,
  virtualTimeline: RefObject<VirtualTimelineHandle>,
  atBottom: RefObject<boolean>,
) {
  const cache = useRef(new Map<string, CachedHistory>());
  const [loaded, setLoaded] = useState<CachedHistory | null>(null);
  const [historyError, setHistoryError] = useState("");
  const [loading, setLoading] = useState(false);
  const [retry, setRetry] = useState(0);
  const [target, setTarget] = useState<number | undefined>();
  const active = useRef(selected);
  active.current = selected;
  const current = useRef(loaded);
  current.current = loaded;
  const request = useRef<AbortController | null>(null);
  const unread = state.unread[selected];
  const id =
    state.private_access[selected]?.id ??
    state.rooms.find((room) => room.name === selected)?.id;
  const historyReady =
    !!unread &&
    loaded?.view === selected &&
    loaded.id === id &&
    loaded.revision === unread.revision;
  function commit(page: CachedHistory) {
    const metadata = currentState.current.unread[page.view];
    if (metadata)
      page = { ...page, through: metadata.through, oldest: metadata.oldest };
    storeHistory(cache.current, page);
    current.current = page;
    setLoaded(page);
    ingest(currentState.current, ledger.current, page.messages);
  }
  async function fetchPage(range: HistoryRange, direction?: "older" | "newer") {
    const view = active.current;
    const snapshot = currentState.current;
    const conversationId =
      snapshot.private_access[view]?.id ??
      snapshot.rooms.find((room) => room.name === view)?.id;
    if (!conversationId || !snapshot.unread[view]) return;
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setLoading(true);
    setHistoryError("");
    try {
      const page = await api.history(view, controller.signal, range);
      const latest = currentState.current;
      const latestId =
        latest.private_access[view]?.id ??
        latest.rooms.find((room) => room.name === view)?.id;
      if (
        controller.signal.aborted ||
        active.current !== view ||
        latestId !== conversationId
      )
        return;
      if (latest.unread[view]?.revision !== page.revision) {
        setRetry((value) => value + 1);
        return;
      }
      const previous = current.current;
      commit(
        direction &&
          previous?.view === view &&
          previous.id === conversationId &&
          previous.revision === page.revision
          ? mergePage(previous, page, direction)
          : {
              ...page,
              id: conversationId,
              through: latest.unread[view]?.through ?? 0,
              oldest: latest.unread[view]?.oldest ?? 0,
            },
      );
    } catch (error) {
      if (!controller.signal.aborted) setHistoryError(errorMessage(error));
    } finally {
      if (request.current === controller) {
        request.current = null;
        setLoading(false);
      }
    }
  }
  useEffect(() => {
    // Revocation and resource identity reuse cannot retain accessible cached history.
    for (const [view, page] of cache.current) {
      const accessId =
        state.private_access[view]?.id ??
        state.rooms.find((room) => room.name === view)?.id;
      if (!accessId || accessId !== page.id) cache.current.delete(view);
    }
    request.current?.abort();
    request.current = null;
    setLoading(false);
    if (!unread || !id) {
      setLoaded(null);
      setLoading(false);
      return;
    }
    const cached = cache.current.get(selected);
    if (
      target === undefined &&
      cached?.id === id &&
      cached.revision === unread.revision
    ) {
      commit(cached);
      return;
    }
    const previous = current.current;
    if (
      target === undefined &&
      previous?.view === selected &&
      previous.id === id &&
      previous.oldest === unread.oldest
    ) {
      const last = previous.through;
      const additions = tail.filter((message) => message.sequence > last);
      if (
        additions.length &&
        previous.revision + additions.length === unread.revision
      ) {
        if (previous.has_after) {
          commit({ ...previous, revision: unread.revision });
          return;
        }
        const merged = mergePage(
          previous,
          {
            view: selected,
            messages: tail,
            revision: unread.revision,
            has_before: previous.has_before,
            has_after: false,
          },
          "newer",
        );
        commit(merged);
        return;
      }
    }
    // Refresh only the current window after edits/reactions/cleanup; do not download earlier history.
    const anchor =
      target ??
      (previous?.view === selected && !atBottom.current
        ? virtualTimeline.current?.anchor()
        : undefined);
    fetchPage(anchor === undefined ? {} : { around: anchor }).catch(
      (error: unknown) => setHistoryError(errorMessage(error)),
    );
    return () => request.current?.abort();
  }, [selected, id, unread?.revision, state.policy_revision, retry, target]);
  useEffect(() => {
    setTarget(undefined);
  }, [selected]);
  return {
    unread,
    historyReady,
    messages:
      loaded?.view === selected && loaded.id === id ? loaded.messages : tail,
    historyError,
    loading,
    refreshing:
      loading &&
      target === undefined &&
      loaded?.view === selected &&
      loaded.id === id &&
      loaded.revision !== unread?.revision,
    hasBefore: loaded?.view === selected && loaded.has_before,
    hasAfter: loaded?.view === selected && loaded.has_after,
    retryHistory: () => setRetry((value) => value + 1),
    loadOlder: () => {
      const first = current.current?.messages[0];
      if (!request.current && first && current.current?.has_before)
        fetchPage({ before: first.sequence }, "older").catch((error: unknown) =>
          setHistoryError(errorMessage(error)),
        );
    },
    loadNewer: () => {
      const last = current.current?.messages.at(-1);
      if (!request.current && last && current.current?.has_after)
        fetchPage({ after: last.sequence }, "newer").catch((error: unknown) =>
          setHistoryError(errorMessage(error)),
        );
    },
    loadLatest: () => {
      setTarget(undefined);
      setLoaded(null);
      current.current = null;
      cache.current.delete(active.current);
      setRetry((value) => value + 1);
    },
    loadSequence: (sequence: number) => {
      setTarget(sequence);
      setRetry((value) => value + 1);
    },
  };
}

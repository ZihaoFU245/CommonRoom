import type { RefObject } from "preact";
import type { VirtualTimelineHandle } from "./useVirtualTimeline.ts";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type {
  HistoryRange,
  Message,
  Snapshot,
  Ledger,
} from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api, ApiError } from "../api/client.ts";
import { ingest } from "../features/conversation/console.ts";
import {
  mergePage,
  HistoryCache,
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
  const cache = useRef(new HistoryCache());
  const [loaded, setLoaded] = useState<CachedHistory | null>(null);
  const [historyError, setHistoryError] = useState("");
  const [loading, setLoading] = useState(false);
  const [retry, setRetry] = useState(0);
  const [jump, setJump] = useState<
    { view: string; sequence: number } | undefined
  >();
  const target = jump?.view === selected ? jump.sequence : undefined;
  const active = useRef(selected);
  active.current = selected;
  const current = useRef(loaded);
  current.current = loaded;
  const request = useRef<AbortController | null>(null);
  const paused = useRef(false);
  const lastRange = useRef<{
    range: HistoryRange;
    direction?: "older" | "newer";
  }>({ range: {} });
  // Reuse ledger work across composer/status renders while keeping live tails
  // available independently of paging success.
  useMemo(
    () =>
      ingest(
        state,
        ledger.current,
        loaded?.view === selected ? loaded.messages : [],
      ),
    [state.rooms, state.direct, loaded?.messages, loaded?.view, selected],
  );
  const unread = state.unread[selected];
  const id =
    state.private_access[selected]?.id ??
    state.rooms.find((room) => room.name === selected)?.id;
  const historyReady =
    !!unread && loaded?.view === selected && loaded.id === id;
  function commit(page: CachedHistory) {
    const metadata = currentState.current.unread[page.view];
    if (metadata)
      page = { ...page, through: metadata.through, oldest: metadata.oldest };
    cache.current.store(page);
    current.current = page;
    setLoaded(page);
    ingest(currentState.current, ledger.current, page.messages);
  }
  async function fetchPage(range: HistoryRange, direction?: "older" | "newer") {
    paused.current = false;
    lastRange.current =
      direction === undefined ? { range } : { range, direction };
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
      let page;
      for (let attempt = 0; attempt < 3; attempt++) {
        try {
          page = await api.history(view, controller.signal, range);
          if (page.view !== view)
            throw new Error(
              "History response belongs to another conversation.",
            );
          if (currentState.current.unread[view]?.revision === page.revision)
            break;
        } catch (error) {
          if (
            !(error instanceof ApiError && error.status === 409) ||
            attempt === 2
          )
            throw error;
        }
        if (controller.signal.aborted) return;
      }
      if (!page)
        throw new Error("History is changing quickly. Retry the current page.");
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
        paused.current = true;
        setHistoryError(
          "History is changing quickly. Showing the last received page; retry to refresh.",
        );
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
      // A jump applies to this fetch only. Later refreshes use the visible
      // window's anchor, and live appends can resume their merge path.
      if (range.around !== undefined)
        setJump((pending) =>
          pending?.view === view && pending.sequence === range.around
            ? undefined
            : pending,
        );
    } catch (error) {
      if (!controller.signal.aborted) {
        if (error instanceof ApiError && error.status === 409)
          paused.current = true;
        setHistoryError(errorMessage(error));
      }
    } finally {
      if (request.current === controller) {
        request.current = null;
        setLoading(false);
      }
    }
  }
  useEffect(() => {
    paused.current = false;
    setJump(undefined);
    setHistoryError("");
    request.current?.abort();
    request.current = null;
    return () => {
      request.current?.abort();
      request.current = null;
    };
  }, [selected, id, state.policy_revision]);
  useEffect(() => {
    // Revocation and resource identity reuse cannot retain accessible cached history.
    for (const [view, page] of cache.current) {
      const accessId =
        state.private_access[view]?.id ??
        state.rooms.find((room) => room.name === view)?.id;
      if (!accessId || accessId !== page.id) cache.current.delete(view);
    }
    if (request.current || paused.current) return;
    setHistoryError("");
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
  }, [selected, id, unread?.revision, state.policy_revision, retry, target]);
  const messages = useMemo(
    () =>
      loaded?.view === selected && loaded.id === id
        ? loaded.has_after
          ? loaded.messages
          : [
              ...loaded.messages,
              ...tail.filter(
                (message) =>
                  message.sequence > (loaded.messages.at(-1)?.sequence ?? 0),
              ),
            ].slice(-1000)
        : tail,
    [loaded, selected, id, tail],
  );
  return {
    unread,
    historyReady,
    messages,
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
    retryHistory: () => {
      const { range, direction } = lastRange.current;
      fetchPage(range, direction).catch((error: unknown) =>
        setHistoryError(errorMessage(error)),
      );
    },
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
      request.current?.abort();
      request.current = null;
      paused.current = false;
      setJump(undefined);
      setLoaded(null);
      current.current = null;
      cache.current.delete(active.current);
      setRetry((value) => value + 1);
    },
    loadSequence: (sequence: number) => {
      paused.current = false;
      request.current?.abort();
      request.current = null;
      setJump({ view: active.current, sequence });
      setRetry((value) => value + 1);
    },
  };
}

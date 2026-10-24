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
import { HistoryLookahead } from "../features/conversation/historyLookahead.ts";
import { ingest } from "../features/conversation/console.ts";
import {
  mergePage,
  previewPage,
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
  nameColors = false,
) {
  const cache = useRef(new HistoryCache());
  const lookahead = useRef(new HistoryLookahead());
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
  const request = useRef<AbortController | null>(null);
  const paused = useRef(false);
  const lastRange = useRef<{
    range: HistoryRange;
    direction?: "older" | "newer";
  }>({ range: {} });
  const unread = state.unread[selected];
  const id =
    state.private_access[selected]?.id ??
    state.rooms.find((room) => room.name === selected)?.id;
  const completeTail = useMemo<CachedHistory | null>(() => {
    if (!unread || !id) return null;
    const complete =
      tail.length === 0
        ? unread.through === 0
        : tail[0]?.sequence === unread.oldest &&
          tail.at(-1)?.sequence === unread.through;
    return complete
      ? {
          view: selected,
          id,
          messages: tail,
          revision: unread.revision,
          through: unread.through,
          oldest: unread.oldest,
          has_before: false,
          has_after: false,
        }
      : null;
  }, [selected, id, tail, unread?.revision, unread?.oldest, unread?.through]);
  const cached = cache.current.get(selected);
  const candidate =
    loaded?.view === selected && loaded.id === id
      ? loaded
      : cached && cached.id === id && unread
        ? cached
        : completeTail;
  const visible = useMemo(
    () => (candidate && unread ? previewPage(candidate, unread, tail) : null),
    [candidate, unread?.revision, unread?.oldest, unread?.through, tail],
  );
  // Cached content is available on the selection render, before effects or HTTP.
  current.current = visible;
  const historyReady = !!unread && visible !== null;
  // Selection must ingest the immediately visible cached window as well.
  useMemo(
    () => ingest(state, ledger.current, visible?.messages ?? []),
    [state.rooms, state.direct, visible?.messages],
  );
  function commit(page: CachedHistory) {
    const metadata = currentState.current.unread[page.view];
    if (metadata)
      page = { ...page, through: metadata.through, oldest: metadata.oldest };
    cache.current.store(page);
    current.current = page;
    setLoaded(page);
    ingest(currentState.current, ledger.current, page.messages);
  }
  function cancelRequest() {
    request.current?.abort();
    request.current = null;
    setLoading(false);
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
    cancelRequest();
    const controller = new AbortController();
    request.current = controller;
    setLoading(true);
    setHistoryError("");
    try {
      let page;
      for (let attempt = 0; attempt < 3; attempt++) {
        try {
          const started = performance.now();
          page = await api.history(view, controller.signal, range, nameColors);
          lookahead.current.sample(performance.now() - started);
          if (page.view !== view)
            throw new Error(
              "History response belongs to another conversation.",
            );
          if (
            currentState.current.unread[view]?.revision === page.revision &&
            (!nameColors ||
              !page.name_colors ||
              page.name_colors.version === currentState.current.color_version)
          )
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
        if (
          error instanceof ApiError &&
          [401, 403, 404].includes(error.status)
        ) {
          cache.current.delete(view);
          current.current = null;
          setLoaded(null);
        }
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
    lookahead.current.reset();
    setJump(undefined);
    setHistoryError("");
    cancelRequest();
    return () => {
      request.current?.abort();
      request.current = null;
    };
  }, [selected, id, state.policy_revision, nameColors]);
  function colorsReady(page: CachedHistory): boolean {
    return (
      !nameColors ||
      state.color_version === undefined ||
      page.name_colors?.version === state.color_version ||
      page.messages.every((message) =>
        tail.some((item) => item.id === message.id),
      )
    );
  }
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
    if (
      target === undefined &&
      visible?.id === id &&
      visible.revision === unread.revision &&
      colorsReady(visible)
    ) {
      commit(visible);
      return;
    }
    const previous = current.current;
    if (
      target === undefined &&
      previous?.view === selected &&
      previous.id === id &&
      previous.oldest === unread.oldest &&
      colorsReady(previous)
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
  }, [
    selected,
    id,
    unread?.revision,
    state.policy_revision,
    state.color_version,
    nameColors,
    retry,
    target,
  ]);
  const messages = useMemo(
    () =>
      visible !== null
        ? visible.has_after
          ? visible.messages
          : [
              ...visible.messages,
              ...tail.filter(
                (message) =>
                  message.sequence > (visible.messages.at(-1)?.sequence ?? 0),
              ),
            ].slice(-1000)
        : tail,
    [visible, tail],
  );
  return {
    unread,
    lookahead: lookahead.current,
    historyReady,
    messages,
    nameColors: visible?.name_colors,
    historyError,
    loading,
    refreshing: loading && target === undefined && visible !== null,
    hasBefore: visible?.has_before ?? false,
    hasAfter: visible?.has_after ?? false,
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
      cancelRequest();
      paused.current = false;
      setJump(undefined);
      setLoaded(null);
      current.current = null;
      cache.current.delete(active.current);
      setRetry((value) => value + 1);
    },
    loadSequence: (sequence: number) => {
      paused.current = false;
      cancelRequest();
      setJump({ view: active.current, sequence });
      setRetry((value) => value + 1);
    },
  };
}

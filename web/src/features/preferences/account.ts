import { defaultSeeds } from "./palette.ts";
import { loadFonts, loadSeeds, loadTheme } from "./settings.ts";
import {
  loadLatex,
  loadNameColors,
  loadMessageBars,
  loadMessageBarOpacity,
  loadNameBorders,
} from "./formatting.ts";
import { loadMarkdown } from "./markdown.ts";
import { loadMarkdownHtml } from "./security.ts";

export interface PreferenceState {
  values: Record<string, string>;
  sync: { instance: string; revision: number };
}
type Transport = (
  values: Record<string, string | null>,
  initialize: boolean,
) => Promise<PreferenceState>;
let generation = 0;
const listeners = new Set<() => void>();
let active:
  | {
      id: string;
      state: PreferenceState;
      pending: Record<string, string>;
      sending: Record<string, string>;
      transport: Transport;
      busy: boolean;
      error: string;
    }
  | undefined;

export function subscribePreferences(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
function notify(): void {
  for (const listener of listeners) listener();
}

export function preferenceValue(key: string, fallback: string): string {
  return (
    active?.pending[key] ??
    active?.sending[key] ??
    active?.state.values[key] ??
    fallback
  );
}
export function preferenceError(): string {
  return active?.error ?? "";
}

function cacheKey(id: string): string {
  const deployment =
    typeof document === "undefined" ? "" : new URL(".", document.baseURI).href;
  return `commonroom-preferences:${deployment}${id}`;
}
function cache(): void {
  if (!active) return;
  try {
    localStorage.setItem(
      cacheKey(active.id),
      JSON.stringify(active.state.values),
    );
  } catch {
    /* Server settings remain authoritative without browser storage. */
  }
}
export function receivePreferences(state: PreferenceState): void {
  if (!active) return;
  const previous = active.state.sync;
  if (
    state.sync.instance === previous.instance &&
    state.sync.revision < previous.revision
  ) {
    notify();
    return;
  }
  active.state = state;
  cache();
  notify();
}

export async function flushPreferences(): Promise<void> {
  const session = active;
  if (!session || session.busy || !Object.keys(session.pending).length) return;
  session.busy = true;
  const sent = session.pending;
  session.pending = {};
  session.sending = sent;
  try {
    const state = await session.transport(sent, false);
    if (active !== session) return;
    session.sending = {};
    session.error = "";
    receivePreferences(state);
  } catch {
    if (active !== session) return;
    session.pending = { ...sent, ...session.pending };
    session.sending = {};
    session.error = "Settings could not sync. Retrying when connected.";
    notify();
    return;
  } finally {
    session.busy = false;
  }
  if (active === session) await flushPreferences();
}
export function changePreference(key: string, value: string): void {
  if (!active || preferenceValue(key, "") === value) return;
  active.pending[key] = value;
  notify();
  // Coalesce slider/color input into one bounded patch.
  const session = active;
  setTimeout(() => {
    if (active === session) flushPreferences().catch(() => {});
  }, 150);
}

function localValues(): Record<string, string> {
  let fonts = { chat: 16, ui: 14 };
  let seeds = { ...defaultSeeds };
  let theme = "system";
  let external = false;
  let notifications = false;
  try {
    fonts = loadFonts(localStorage);
    seeds = loadSeeds(localStorage);
    theme = loadTheme(localStorage);
    external = localStorage.getItem("commonroom-external-avatars") === "true";
    notifications = localStorage.getItem("commonroom-notifications") === "on";
  } catch {
    /* Use validated defaults when storage is blocked. */
  }
  return {
    theme,
    "palette.base": seeds.base,
    "palette.accent": seeds.accent,
    "font.chat": String(fonts.chat),
    "font.ui": String(fonts.ui),
    markdown: String(loadMarkdown()),
    latex: String(loadLatex()),
    name_colors: String(loadNameColors()),
    message_bars: String(loadMessageBars()),
    message_bar_opacity: String(loadMessageBarOpacity()),
    name_borders: String(loadNameBorders()),
    allow_html: String(loadMarkdownHtml()),
    external_avatars: String(external),
    notifications: String(notifications),
  };
}

/** The server fills each missing key atomically; established values always win. */
export async function connectPreferences(
  id: string,
  transport: Transport,
): Promise<void> {
  disconnectPreferences();
  const attempt = generation;
  const local = localValues();
  try {
    const cached: unknown = JSON.parse(
      localStorage.getItem(cacheKey(id)) ?? "null",
    );
    if (typeof cached === "object" && cached !== null) {
      for (const [key, value] of Object.entries(cached)) {
        if (Object.hasOwn(local, key) && typeof value === "string")
          local[key] = value;
      }
    }
  } catch {
    /* Legacy values seed a newly used account. */
  }
  const state = await transport(local, true);
  if (attempt !== generation)
    throw new Error("Settings session changed. Please sign in again.");
  active = {
    id,
    transport,
    state,
    pending: {},
    sending: {},
    busy: false,
    error: "",
  };
  cache();
  notify();
}
export function disconnectPreferences(): void {
  generation++;
  active = undefined;
  notify();
}

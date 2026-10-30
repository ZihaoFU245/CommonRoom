import { useEffect, useState } from "preact/hooks";
import type { Snapshot } from "../api/protocol.ts";
import {
  flushPreferences,
  preferenceError,
  receivePreferences,
  subscribePreferences,
} from "../features/preferences/account.ts";

export function usePreferenceSync(state: Snapshot): string {
  const [error, setError] = useState(preferenceError);
  useEffect(() => subscribePreferences(() => setError(preferenceError())), []);
  useEffect(() => {
    if (state.preferences !== undefined && state.sync !== undefined) {
      receivePreferences({ values: state.preferences, sync: state.sync });
    }
  }, [state.preferences, state.sync]);
  useEffect(() => {
    const retry = () => {
      flushPreferences().catch(() => {});
    };
    const timer = window.setInterval(retry, 5000);
    window.addEventListener("online", retry);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("online", retry);
    };
  }, []);
  return error;
}

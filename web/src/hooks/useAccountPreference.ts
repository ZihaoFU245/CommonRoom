import { useEffect, useState } from "preact/hooks";
import {
  changePreference,
  preferenceValue,
  subscribePreferences,
} from "../features/preferences/account.ts";

export function useAccountPreference(
  key: string,
  fallback: string | (() => string),
): readonly [string, (value: string) => void] {
  const [defaultValue] = useState(fallback);
  const [value, setValue] = useState(() => preferenceValue(key, defaultValue));
  useEffect(() => {
    const update = () => setValue(preferenceValue(key, defaultValue));
    update();
    return subscribePreferences(update);
  }, [key, defaultValue]);
  return [value, (next) => changePreference(key, next)];
}

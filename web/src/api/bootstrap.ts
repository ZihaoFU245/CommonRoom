import { api } from "./client.ts";
import { loadNameColors } from "../features/preferences/formatting.ts";
import {
  connectPreferences,
  preferenceValue,
} from "../features/preferences/account.ts";

/** Seed unset settings before mounting authenticated controls. */
export async function loadInitialAccount() {
  const requestedColors = loadNameColors();
  const account = await api.me(requestedColors);
  if (account.preferences !== undefined) {
    const preferences = await connectPreferences(
      account.account_access.id,
      api.preferences,
      account.sync
        ? { values: account.preferences, sync: account.sync }
        : undefined,
    );
    const colors = preferenceValue("name_colors", "false") === "true";
    if (
      colors === requestedColors &&
      account.sync?.instance === preferences.sync.instance &&
      account.sync.revision === preferences.sync.revision
    )
      return account;
    return api.me(colors);
  }
  return account;
}

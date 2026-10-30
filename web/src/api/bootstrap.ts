import { api } from "./client.ts";
import { loadNameColors } from "../features/preferences/formatting.ts";
import {
  connectPreferences,
  preferenceValue,
} from "../features/preferences/account.ts";

/** Seed unset settings before mounting authenticated controls. */
export async function loadInitialAccount() {
  const account = await api.me(loadNameColors());
  if (account.preferences !== undefined) {
    await connectPreferences(account.account_access.id, api.preferences);
    return api.me(preferenceValue("name_colors", "false") === "true");
  }
  return account;
}

import { api } from "./client.ts";
import { loadNameColors } from "../features/preferences/formatting.ts";
import { connectPreferences } from "../features/preferences/account.ts";

/** Mount from the first authorized response. Seed unset settings concurrently. */
export async function loadInitialAccount() {
  const requestedColors = loadNameColors();
  const room =
    typeof location === "undefined"
      ? undefined
      : (new URLSearchParams(location.hash.slice(1)).get("room") ?? undefined);
  const account = await api.me(requestedColors, { room });
  if (account.preferences !== undefined) {
    await connectPreferences(
      account.account_access.id,
      api.preferences,
      account.sync
        ? { values: account.preferences, sync: account.sync }
        : undefined,
      true,
    );
  }
  return account;
}

import { api } from "./client.ts";
import { loadNameColors } from "../features/preferences/formatting.ts";

/** Use the saved projection preference for the first authenticated paint. */
export function loadInitialAccount() {
  return api.me(loadNameColors());
}

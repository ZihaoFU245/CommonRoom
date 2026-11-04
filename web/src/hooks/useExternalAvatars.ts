import { readStorage } from "../api/browserStorage.ts";
import { useAccountPreference } from "./useAccountPreference.ts";
export function useExternalAvatars(): [boolean, (enabled: boolean) => void] {
  const [value, set] = useAccountPreference("external_avatars", () =>
    String(readStorage("commonroom-external-avatars") === "true"),
  );
  return [value === "true", (enabled) => set(String(enabled))];
}

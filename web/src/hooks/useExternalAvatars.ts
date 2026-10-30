import { useAccountPreference } from "./useAccountPreference.ts";
export function useExternalAvatars(): [boolean, (enabled: boolean) => void] {
  const [value, set] = useAccountPreference("external_avatars", () => {
    try {
      return String(
        localStorage.getItem("commonroom-external-avatars") === "true",
      );
    } catch {
      return "false";
    }
  });
  return [value === "true", (enabled) => set(String(enabled))];
}

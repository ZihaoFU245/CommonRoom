export const settingsSections = {
  account: "Account",
  appearance: "Appearance",
  alias: "Alias",
  preferences: "Preferences",
  security: "Security",
} as const;

export type SettingsSection = keyof typeof settingsSections;
export type SettingsScreen = "index" | SettingsSection;

export function settingsScreen(
  params: URLSearchParams,
): SettingsScreen | undefined {
  const screen = params.get("settings");
  if (screen === "index" || (screen && Object.hasOwn(settingsSections, screen)))
    return screen as SettingsScreen;
  return undefined;
}

export function settingsDepth(screen: SettingsScreen | undefined) {
  return screen === undefined ? 0 : screen === "index" ? 1 : 2;
}

import type { Message, MessageColors } from "../../api/protocol.ts";

/** Must match the server's accepted names; style.css maps each to a shade per theme. */
export const nameColors = [
  "red",
  "orange",
  "amber",
  "yellow",
  "lime",
  "green",
  "emerald",
  "teal",
  "cyan",
  "sky",
  "blue",
  "indigo",
  "violet",
  "purple",
  "fuchsia",
  "pink",
] as const;
export type NameColor = (typeof nameColors)[number];

export function isNameColor(value: unknown): value is NameColor {
  return nameColors.some((color) => color === value);
}

/** An unknown or missing entry falls back to the default name color. */
export function nameColorOf(
  colors: Record<string, string>,
  name: string,
): NameColor | undefined {
  const color = colors[name];
  return isNameColor(color) ? color : undefined;
}

export function nameColorLabel(color: NameColor): string {
  return color.charAt(0).toUpperCase() + color.slice(1);
}

/** Room overrides replace global colors for the names they cover. */
export function scopedColors(
  global: Record<string, string> | undefined,
  room: Record<string, string> | undefined,
): Record<string, string> {
  return room ? { ...global, ...room } : (global ?? {});
}

/** Keeps one object identity while the mapping is unchanged, so memoized rows skip rendering. */
export function sameColors(
  previous: Record<string, string>,
  next: Record<string, string>,
): boolean {
  const keys = Object.keys(next);
  return (
    keys.length === Object.keys(previous).length &&
    keys.every((key) => previous[key] === next[key])
  );
}

/** Page colors are resolved on the server and expire with color-directory or restart changes. */
export function resolvedColors(
  base: Record<string, string>,
  version: string | undefined,
  ...profiles: (MessageColors | undefined)[]
): Record<string, string> {
  const result = new Map(Object.entries(base));
  for (const profile of profiles) {
    if (!profile || profile.version !== version) continue;
    for (const [name, color] of Object.entries(profile.values)) {
      if (color === null) result.delete(name);
      else result.set(name, color);
    }
  }
  return Object.fromEntries(result);
}

/** Keep supplemental data bounded by the disclosed names in the retained window. */
export function mergeMessageColors(
  previous: MessageColors | undefined,
  next: MessageColors | undefined,
  messages: Message[],
): MessageColors | undefined {
  const version = next?.version ?? previous?.version;
  if (version === undefined) return undefined;
  const values = {
    ...(previous?.version === version ? previous.values : {}),
    ...next?.values,
  };
  const names = new Set(
    messages.flatMap((message) => [
      message.from,
      ...message.mentions,
      ...(message.reply ? [message.reply.from] : []),
    ]),
  );
  return {
    version,
    values: Object.fromEntries(
      Object.entries(values).filter(([name]) => names.has(name)),
    ),
  };
}

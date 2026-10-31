import type { Message, MessageDisplayNames } from "../../api/protocol.ts";

export function resolveDisplayNames(
  base: Record<string, string>,
  version: string | undefined,
  ...profiles: (MessageDisplayNames | undefined)[]
): Record<string, string> {
  const result = new Map(Object.entries(base));
  for (const profile of profiles) {
    if (!profile || profile.version !== version) continue;
    for (const [name, value] of Object.entries(profile.values)) {
      if (value === null) result.delete(name);
      else result.set(name, value);
    }
  }
  return Object.fromEntries(result);
}
export function mergeDisplayNames(
  previous: MessageDisplayNames | undefined,
  next: MessageDisplayNames | undefined,
  messages: Message[],
): MessageDisplayNames | undefined {
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

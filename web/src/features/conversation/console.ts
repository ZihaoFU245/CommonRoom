import type {
  Snapshot,
  Message,
  Ledger,
  LocalOutput,
  TimelineEntry,
  Command,
  Hint,
} from "../../api/protocol.ts";
// Commands and output live only in this tab; chat comes from server snapshots.
export function ingest(
  snapshot: Pick<Snapshot, "rooms" | "direct">,
  ledger: Ledger = { sequence: 0, messages: new Map() },
  extras: Message[] = [],
) {
  const messages = [
    ...new Map(
      [
        ...extras,
        ...snapshot.rooms.flatMap((r) => r.messages),
        ...snapshot.direct,
      ].map((message) => [message.id, message]),
    ).values(),
  ].sort((a, b) => a.sequence - b.sequence || a.time - b.time);
  const current = new Set(messages.map((message) => message.id));
  for (const key of ledger.messages.keys()) {
    if (!current.has(key)) ledger.messages.delete(key);
  }
  // Insert fetched older history before known messages without moving local
  // command output. New live messages still append to the tab's ledger.
  for (let i = 0; i < messages.length;) {
    const current = messages[i];
    if (!current) break;
    if (ledger.messages.has(current.id)) {
      i++;
      continue;
    }
    const start = i;
    while (i < messages.length) {
      const next = messages[i];
      if (!next || ledger.messages.has(next.id)) break;
      i++;
    }
    const previous = messages[start - 1];
    const next = messages[i];
    const left = previous ? (ledger.messages.get(previous.id) ?? 0) : 0;
    const right = next ? (ledger.messages.get(next.id) ?? null) : null;
    for (let j = start; j < i; j++) {
      const message = messages[j];
      if (!message) continue;
      const order =
        right === null
          ? ++ledger.sequence
          : left + ((right - left) * (j - start + 1)) / (i - start + 1);
      ledger.messages.set(message.id, order);
    }
  }
  return ledger;
}

export function timeline(
  messages: Message[],
  output: LocalOutput[],
  ledger: Ledger,
  cleared = 0,
  view?: string,
): TimelineEntry[] {
  return [
    ...messages.map((message) => ({
      kind: "message" as const,
      key: message.id,
      message,
      order: ledger.messages.get(message.id) ?? 0,
    })),
    ...output.filter((entry) => view === undefined || entry.room === view),
  ]
    .filter((entry) => entry.order > cleared)
    .sort((a, b) => a.order - b.order);
}

export function privatePeers(
  snapshot: Pick<Snapshot, "username" | "direct"> &
    Partial<Pick<Snapshot, "private_peers">>,
): string[] {
  if (snapshot.private_peers) return [...snapshot.private_peers].sort();
  return [
    ...new Set(
      [
        ...snapshot.direct.map((message) =>
          message.from === snapshot.username ? message.to : message.from,
        ),
      ].filter((peer): peer is string => typeof peer === "string"),
    ),
  ].sort();
}

export function privateMessages(
  messages: Message[],
  self: string,
  peer: string | null,
  privateId?: string,
) {
  return messages.filter((message) =>
    privateId && message.private_id
      ? message.private_id === privateId
      : (message.from === self && message.to === peer) ||
        (message.from === peer && message.to === self),
  );
}
export function redactCommand(text: string) {
  if (/^\/passwd(?:\s|$)/.test(text)) return "/passwd •••• ••••";
  return text.replace(/^(\/(?:user|reset)\s+\S+\s+)\S+/, "$1••••");
}
export function helpSections(text: string) {
  const groups: {
    title: string;
    commands: { usage: string; description: string }[];
  }[] = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    if (/^\[.+\]$/.test(line))
      groups.push({ title: line.slice(1, -1), commands: [] });
    else {
      if (!groups.length) groups.push({ title: "Commands", commands: [] });
      const [usage = "", description] = line.split(" — ");
      groups.at(-1)?.commands.push({ usage, description: description || "" });
    }
  }
  return groups;
}

export function suggestions(
  draft: string,
  commands: Command[],
  users: string[],
  rooms: string[],
): Hint[] {
  if (!draft.startsWith("/") || draft.includes("\n")) return [];
  const parts = draft.split(/\s+/);
  const name = parts[0] || "";
  const describe = (command: Command) =>
    command.requirements
      ? `${command.description} · Requires: ${command.requirements}`
      : command.description;
  if (parts.length === 1) {
    return commands
      .filter((c) => c.name.startsWith(name))
      .map((c) => ({
        label: c.usage,
        description: describe(c),
        value: c.name + (c.usage === c.name ? "" : " "),
      }));
  }
  const spec = commands.find((c) => c.name === name);
  if (!spec) return [];
  const position = parts.length - 1;
  const prefix = parts.at(-1) || "";
  if (["/grant", "/revoke"].includes(name)) {
    if (
      position === 3 &&
      (prefix.startsWith("/") ||
        !["user", "admin", "su"].includes(parts[2] ?? ""))
    ) {
      return commands
        .filter((command) => command.name.startsWith(prefix))
        .map((command) => ({
          label: command.name,
          description: describe(command),
          value: [...parts.slice(0, 3), command.name].join(" ") + " ",
        }));
    }
    const granted = commands.find((command) => command.name === parts[3]);
    if (position > 3 && granted)
      return [
        { label: granted.usage, description: describe(granted), value: null },
      ];
  }
  let values: string[] = [];
  if (
    [
      "/tell",
      "/grant",
      "/revoke",
      "/add",
      "/kick",
      "/reset",
      "/disable",
      "/enable",
      "/deleteuser",
    ].includes(name) &&
    position === 1
  )
    values = users;
  if (
    ["/join", "/leave", "/delete", "/members"].includes(name) &&
    position === 1
  )
    values = rooms;
  if (["/add", "/kick"].includes(name) && position === 2) values = rooms;
  if (["/grant", "/revoke"].includes(name) && position === 2)
    values = [
      ...new Set([
        "user",
        "admin",
        "su",
        ...rooms,
        "@global",
        "@account:",
        "@private:",
      ]),
    ];
  if (
    ["/grant", "/revoke"].includes(name) &&
    position === 3 &&
    ["user", "admin", "su"].includes(parts[2] ?? "")
  )
    values = rooms;
  if (name === "/man" && position === 1)
    values = [
      ...new Set([
        "grant",
        "revoke",
        "permissions",
        "groups",
        "scopes",
        "ownership",
        ...commands.map((command) => command.name.slice(1)),
      ]),
    ];
  if (name === "/clean" && position === 2)
    values = [...rooms, "@private", "@all"];
  if (values.length)
    return values
      .filter((value) => value.startsWith(prefix))
      .map((value) => ({
        label: value,
        description: spec.requirements
          ? `${spec.usage} · Requires: ${spec.requirements}`
          : spec.usage,
        value: [...parts.slice(0, -1), value].join(" ") + " ",
      }));
  return [{ label: spec.usage, description: describe(spec), value: null }];
}

import type {
  Snapshot,
  Message,
  Ledger,
  LocalOutput,
  TimelineEntry,
  Command,
  Hint,
} from "../../api/protocol.ts";
import { permissionNames } from "./completion.ts";
// Commands and output live only in this tab; chat comes from server snapshots.
export function clearedOrder(cleared: Record<string, number>, view: string) {
  return Object.hasOwn(cleared, view) ? (cleared[view] ?? 0) : 0;
}
const ingestIds = new WeakMap<Ledger, Set<string>>();
const noExtras: Message[] = [];
export function ingest(
  snapshot: Pick<Snapshot, "rooms" | "direct">,
  ledger: Ledger = { sequence: 0, messages: new Map() },
  extras: Message[] = noExtras,
) {
  const current = ingestIds.get(ledger) ?? new Set<string>();
  ingestIds.set(ledger, current);
  current.clear();
  const messages: Message[] = [];
  let additions = false;
  function collect(source: Message[]) {
    for (const message of source) {
      if (current.has(message.id)) continue;
      current.add(message.id);
      messages.push(message);
      if (!ledger.messages.has(message.id)) additions = true;
    }
  }
  collect(extras);
  for (const room of snapshot.rooms) collect(room.messages);
  collect(snapshot.direct);
  for (const key of ledger.messages.keys()) {
    if (!current.has(key)) ledger.messages.delete(key);
  }
  if (!additions) return ledger;
  messages.sort((a, b) => a.sequence - b.sequence || a.time - b.time);
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
export function redactCommand(text: string): string {
  let prefix = "";
  let sudo = text.match(/^\/sudo\s+/u)?.[0];
  while (sudo) {
    prefix += sudo;
    text = text.slice(sudo.length);
    sudo = text.match(/^\/sudo\s+/u)?.[0];
  }
  const alias = text.match(/^(\/alias\s+\S+\s+)(.+)$/u);
  if (alias?.[1] && alias[2])
    return (
      prefix +
      alias[1] +
      redactCommand(alias[2].startsWith("/") ? alias[2] : `/${alias[2]}`)
    );
  if (/^\/passwd(?:\s|$)/.test(text)) return prefix + "/passwd •••• ••••";
  return (
    prefix + text.replace(/^(\/(?:new\s+user|reset)\s+\S+\s+)\S+/, "$1••••")
  );
}
export function commandText(text: string) {
  return text.trim().replace(/^\/sudo\s+/u, "");
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
  context: {
    messages?: Message[];
    aliases?: string[];
    privateScopes?: string[];
  } = {},
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
  if (name === "/alias" && parts.length > 3) {
    return suggestions(
      parts.slice(2).join(" "),
      commands.filter((command) => !["/alias", "/sudo"].includes(command.name)),
      users,
      rooms,
      context,
    ).map((hint) => ({
      ...hint,
      value: hint.value ? `${parts.slice(0, 2).join(" ")} ${hint.value}` : null,
    }));
  }
  if (name === "/sudo") {
    return suggestions(
      draft.replace(/^\/sudo\s+/u, ""),
      commands.filter((command) => command.name !== "/sudo"),
      users,
      rooms,
      context,
    ).map((hint) => ({
      ...hint,
      label: `/sudo ${hint.label}`,
      value: hint.value ? `/sudo ${hint.value}` : null,
    }));
  }
  const position = parts.length - 1;
  const prefix = parts.at(-1) || "";
  if (["/grant", "/revoke"].includes(name)) {
    if (
      position === 3 &&
      (prefix.startsWith("/") ||
        !["user", "admin", "su"].includes(parts[2] ?? ""))
    ) {
      return [
        ...commands.map((command) => ({
          value: command.name,
          description: describe(command),
        })),
        ...permissionNames.map((value) => ({
          value,
          description: "Permission",
        })),
      ]
        .filter((permission) => permission.value.startsWith(prefix))
        .map((permission) => ({
          label: permission.value,
          description: permission.description,
          value: [...parts.slice(0, 3), permission.value].join(" ") + " ",
        }));
    }
    const granted = commands.find((command) => command.name === parts[3]);
    if (position > 3 && granted)
      return [
        { label: granted.usage, description: describe(granted), value: null },
      ];
  }
  let values: string[] = [];
  const scopes = [
    ...rooms,
    "@global",
    ...users.map((user) => `@account:${user}`),
    ...(context.privateScopes || []),
  ];
  if (name === "/bot") {
    const operation = parts[1];
    if (position === 1) values = ["new", "ls", "info", "owner", "rm", "token"];
    if (position === 2 && operation === "token") values = ["new", "ls", "rm"];
    if (
      (position === 2 && ["info", "owner", "rm"].includes(operation || "")) ||
      (position === 3 && operation === "token")
    )
      values = users;
    if (
      (position === 2 && operation === "ls") ||
      (position === 3 && ["new", "info"].includes(operation || ""))
    )
      values = [...rooms, "@global"];
    if (position === 3 && operation === "owner") values = users;
    if (position === 3 && operation === "rm") values = ["@global"];
  }
  if (["/debug", "/markdown"].includes(name) && position === 1)
    values = ["on", "off"];
  if (name === "/permissions" && position === 1)
    values = [...scopes, "@groups", "@audit"];
  if (name === "/owner" && position === 1) values = users;
  if (name === "/owner" && position === 2) values = rooms;
  if (name === "/history" && position === 1)
    values = rooms.map((room) => `#${room}`);
  if (
    ["/reply", "/react", "/retract", "/edit"].includes(name) &&
    position === 1
  )
    values = (context.messages || []).map((message) => message.id);
  if (name === "/react" && position === 2)
    values = Object.keys(
      context.messages?.find((message) => message.id === parts[1])?.reactions ||
        {},
    );
  if (name === "/alias" && position === 1) values = context.aliases || [];
  if (name === "/alias" && position === 2)
    values = commands
      .filter((command) => !["/alias", "/sudo"].includes(command.name))
      .map((command) => command.name);
  if (["/new", "/rm", "/ls"].includes(name) && position === 1)
    values =
      name === "/new"
        ? ["room", "user"]
        : name === "/rm"
          ? ["room", "user", "group"]
          : ["room", "user", "group", "@user", "@admin", "@su"];
  if (name === "/new" && parts[1] === "user" && position === 4)
    values = ["user", "admin"];
  if (name === "/rm" && position === 2)
    values = parts[1] === "room" ? rooms : users;
  if (name === "/rm" && parts[1] === "group" && position === 3)
    values = ["user", "admin", "su"];
  if (name === "/rm" && parts[1] === "group" && position === 4) values = rooms;
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
    ].includes(name) &&
    position === 1
  )
    values = users;
  if (name === "/rename" && position === 1)
    values = [...new Set(["room", "user", ...users])];
  if (name === "/rename" && position === 2) {
    if (parts[1] === "room") values = rooms;
    if (parts[1] === "user") values = users;
  }
  if (["/join", "/leave", "/members"].includes(name) && position === 1)
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
        ...users.map((user) => `@account:${user}`),
        ...(context.privateScopes || []),
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
        "alias",
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

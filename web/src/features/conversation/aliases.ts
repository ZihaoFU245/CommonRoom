import type { Command } from "../../api/protocol.ts";
import { redactCommand } from "./console.ts";

export type Aliases = Record<string, string>;
const aliasCommand: Command = {
  name: "/alias",
  usage: "/alias A B",
  description: "Map a command prefix in this browser; /man alias explains",
  requirements: "",
  section: "General",
  admin: false,
  console: false,
};

export function browserCommands(
  commands: Command[],
  aliases: Aliases,
): Command[] {
  return [
    ...commands.filter((command) => !Object.hasOwn(aliases, command.name)),
    aliasCommand,
    ...Object.entries(aliases).map(([name, target]) => ({
      ...aliasCommand,
      name,
      usage: name,
      description: `Alias for ${redactCommand(target)}`,
    })),
  ];
}

export function aliasHelp(aliases: Aliases): string {
  return [
    `[Browser]\n${aliasCommand.usage} — ${aliasCommand.description}`,
    ...Object.entries(aliases).map(
      ([name, target]) => `${name} — Alias for ${redactCommand(target)}`,
    ),
  ].join("\n");
}

export const aliasManual = `# /alias

Map one command prefix in this browser. The server receives the expanded command.

## Usage

\`\`\`
/alias A B
\`\`\`

A is a command name; B is a command with optional subcommands or arguments.
Leading slashes are optional. Repeating A replaces its mapping.

## Examples

\`\`\`
/alias rooms ls room
/rooms
/alias drop rm room
/drop support
/sudo /drop support
\`\`\`

The last two examples expand to /rm room support and /sudo /rm room support.
Expansion happens once and preserves following arguments. Aliases can replace
existing command names; /alias and /sudo cannot be remapped.

## Storage and permissions

Mappings survive refresh and are shared by accounts in this browser profile.
They stay in browser localStorage; clearing this site's storage removes them.
Do not include passwords in mappings: their stored values are plain text.
Aliases do not grant permissions. The expanded command uses the same server
checks and removal/retract confirmations as a directly typed command.
Use /help to see saved mappings, or Settings → Alias to add, edit, or remove them.
This command is available in the web app only.`;

const commandName = /^\/[^\s/]+$/u;
function validAlias(a: string, b: unknown): b is string {
  return (
    commandName.test(a) &&
    !["/alias", "/sudo"].includes(a) &&
    typeof b === "string" &&
    commandName.test(b.split(/\s+/u)[0] ?? "")
  );
}
const key = "commonroom.command-aliases";
export function loadAliases(): Aliases {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(key) ?? "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(
      Object.entries(value).filter(([a, b]) => validAlias(a, b)),
    );
  } catch {
    return {};
  }
}
export function saveAlias(aliases: Aliases, input: string): Aliases {
  const match = input.match(/^\/alias\s+(\S+)\s+(.+)$/u);
  if (!match?.[1] || !match[2])
    throw new Error("Usage: /alias A B (for example /alias rooms /ls room)");
  return setAlias(aliases, match[1], match[2]);
}
export function setAlias(
  aliases: Aliases,
  name: string,
  target: string,
): Aliases {
  const a = name.trim().startsWith("/") ? name.trim() : `/${name.trim()}`;
  const b = target.trim().startsWith("/") ? target.trim() : `/${target.trim()}`;
  if (!validAlias(a, b))
    throw new Error(
      "Use a single command name and a command target; /alias and /sudo cannot be remapped.",
    );
  const next = { ...aliases, [a]: b };
  localStorage.setItem(key, JSON.stringify(next));
  return next;
}
export function removeAlias(aliases: Aliases, name: string): Aliases {
  const next = { ...aliases };
  delete next[name];
  localStorage.setItem(key, JSON.stringify(next));
  return next;
}
export function expandAlias(input: string, aliases: Aliases): string {
  const sudo = input.match(/^\/sudo\s+/u)?.[0] ?? "";
  const text = input.slice(sudo.length);
  const name = text.split(/\s+/u)[0] ?? "";
  return (
    sudo +
    (Object.hasOwn(aliases, name)
      ? (aliases[name] ?? name) + text.slice(name.length)
      : text)
  );
}

import { markdownCommand } from "../preferences/markdown.ts";
import type { Command } from "../../api/protocol.ts";
import { redactCommand } from "./console.ts";
import { inlineCode } from "./manual.ts";

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
const versionCommand: Command = {
  name: "/version",
  usage: "/version",
  description: "Show the UI build version and Git commit",
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
    markdownCommand,
    versionCommand,
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
    `## Browser\n\n- \`${markdownCommand.usage}\` — ${markdownCommand.description}`,
    `- \`${aliasCommand.usage}\` — ${aliasCommand.description}`,
    `- \`${versionCommand.usage}\` — ${versionCommand.description}`,
    ...Object.entries(aliases).map(
      ([name, target]) =>
        `- ${inlineCode(name)} — Alias for ${inlineCode(redactCommand(target))}`,
    ),
  ].join("\n");
}

export const aliasManual = `# /alias

- Map one command prefix in this browser.
- The server receives the expanded command.

## Usage

\`\`\`text
/alias A B
\`\`\`

- A is a command name
- B is a command with optional subcommands or arguments.
- Leading slashes are optional.
- Repeating A replaces its mapping.

## Examples

\`\`\`text
/alias rooms room ls
/rooms
/alias drop room delete
/drop #support
/sudo /drop #support
\`\`\`

- The last two examples expand to /room delete #support and /sudo /room delete #support.
- Expansion happens once and preserves following arguments.
- Aliases can replace existing command names; /alias, /sudo and /markdown cannot be remapped.

## Storage and permissions

- Mappings survive refresh and are shared by accounts in this browser profile.
- They stay in browser localStorage; clearing this site's storage removes them.
- Do not include passwords in mappings: their stored values are plain text.
- Aliases do not grant permissions.
- The expanded command uses the same server checks and removal/retract confirmations as a directly typed command.
- Use /help to see saved mappings, or Settings → Alias to add, edit, or remove them.
- This command is available in the web app only.`;

const commandName = /^\/[^\s/]+$/u;
function validAlias(a: string, b: unknown): b is string {
  return (
    commandName.test(a) &&
    !["/alias", "/sudo", "/markdown"].includes(a) &&
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
    throw new Error("Usage: /alias A B (for example /alias rooms /room ls)");
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
      "Use a single command name and a command target; /alias, /sudo and /markdown cannot be remapped.",
    );
  const next = { ...aliases, [a]: b };
  try {
    localStorage.setItem(key, JSON.stringify(next));
  } catch {
    throw new Error("Could not save aliases in this browser.");
  }
  return next;
}
export function removeAlias(aliases: Aliases, name: string): Aliases {
  const next = { ...aliases };
  delete next[name];
  try {
    localStorage.setItem(key, JSON.stringify(next));
  } catch {
    throw new Error("Could not save aliases in this browser.");
  }
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

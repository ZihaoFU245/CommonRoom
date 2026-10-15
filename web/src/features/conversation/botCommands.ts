import type { BotCommands, Hint } from "../../api/protocol.ts";

/** Bot commands remain ordinary mention-prefixed messages in rooms and DMs. */
export function botCommandSuggestions(
  prefix: string,
  bots: BotCommands[],
): Hint[] {
  const match = /^@(\S+)\s+(.*)$/u.exec(prefix);
  if (!match) return [];
  const bot = bots.find((candidate) => candidate.username === match[1]);
  if (!bot) return [];
  const tokens = (match[2] ?? "").split(/\s+/u);
  const token = tokens.pop() ?? "";
  let commands = bot.commands;
  let path = `@${bot.username}`;
  for (const part of tokens) {
    const command = commands.find((candidate) => candidate.name === part);
    if (!command) return [];
    if (!command.subcommands?.length)
      return [
        {
          label: `@${bot.username} ${command.usage}`,
          description: command.description,
          value: null,
        },
      ];
    commands = command.subcommands;
    path += ` ${command.name}`;
  }
  return commands
    .filter((command) => command.name.startsWith(token))
    .map((command) => ({
      label: `@${bot.username} ${command.usage}`,
      description: command.description,
      value: `${path} ${command.name} `,
      ...(command.subcommands?.length || tokens.length
        ? { advance: true }
        : {}),
    }));
}

import type { Command, Message } from "../../api/protocol.ts";

export function debugAccess(
  permissions: string[],
  elevated: boolean,
  globalPermissions: string[],
  globalCommands: Command[],
) {
  return (
    permissions.includes("r:message.metadata") ||
    (elevated &&
      globalPermissions.includes("x:command.sudo") &&
      globalCommands.some((command) => command.name === "/sudo"))
  );
}

type MessageAuthor = Pick<Message, "from">;

/**
 * Commands advertised under the scope's `/sudo` descriptor. The server only
 * populates `subcommands` when the caller currently holds `/sudo`,
 * `x:command.sudo` at global scope, and the contextual `/sudo` grant.
 */
function elevatedCommands(commands: Command[]): Set<string> {
  const sudo = commands.find((command) => command.name === "/sudo");
  return new Set(sudo?.subcommands?.map((command) => command.name) ?? []);
}

/**
 * Returns the command that performs `action` on this message, preferring the
 * caller's direct grant and falling back to a sudo-wrapped command. Returns
 * null when neither the direct nor the elevated route is authorized.
 */
function actionCommand(
  action: "edit" | "retract",
  message: MessageAuthor,
  username: string,
  permissions: string[],
  commands: Command[],
  elevated: Set<string>,
): string | null {
  const name = `/${action}`;
  const own = message.from === username;
  const direct =
    commands.some((command) => command.name === name) &&
    (permissions.includes(`w:message.${action}.any`) ||
      (own && permissions.includes(`w:message.${action}.own`)));
  if (direct) return name;
  return elevated.has(name) ? `/sudo ${name}` : null;
}

export function messageActionCommand(
  action: "edit" | "retract",
  message: MessageAuthor,
  username: string,
  permissions: string[],
  commands: Command[],
): string | null {
  return actionCommand(
    action,
    message,
    username,
    permissions,
    commands,
    elevatedCommands(commands),
  );
}

export function messageActions(
  message: MessageAuthor,
  username: string,
  permissions: string[],
  commands: Command[],
) {
  const elevated = elevatedCommands(commands);
  const allowed = (name: string) =>
    commands.some((command) => command.name === name);
  return {
    react: permissions.includes("w:message.react") && allowed("/react"),
    reply: permissions.includes("w:message.create") && allowed("/reply"),
    edit:
      actionCommand(
        "edit",
        message,
        username,
        permissions,
        commands,
        elevated,
      ) !== null,
    retract:
      actionCommand(
        "retract",
        message,
        username,
        permissions,
        commands,
        elevated,
      ) !== null,
  };
}

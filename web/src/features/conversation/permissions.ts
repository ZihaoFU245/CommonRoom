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

export function messageActions(
  message: Message,
  username: string,
  permissions: string[],
  commands: Command[],
) {
  const allowed = (name: string) =>
    commands.some((command) => command.name === name);
  return {
    react: permissions.includes("w:message.react") && allowed("/react"),
    reply: permissions.includes("w:message.create") && allowed("/reply"),
    edit:
      allowed("/edit") &&
      (permissions.includes("w:message.edit.any") ||
        (message.from === username &&
          permissions.includes("w:message.edit.own"))),
    retract:
      allowed("/retract") &&
      (permissions.includes("w:message.retract.any") ||
        (message.from === username &&
          permissions.includes("w:message.retract.own"))),
  };
}

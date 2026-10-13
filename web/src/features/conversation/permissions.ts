import type { Command, Message } from "../../api/protocol.ts";

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
      message.from === username &&
      permissions.includes("w:message.edit.own") &&
      allowed("/edit"),
    retract:
      allowed("/retract") &&
      (permissions.includes("w:message.retract.any") ||
        (message.from === username &&
          permissions.includes("w:message.retract.own"))),
  };
}

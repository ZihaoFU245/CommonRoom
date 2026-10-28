import type { Hint } from "../../api/protocol.ts";

export const permissionNames = [
  "r:room.discover",
  "r:message.read",
  "r:message.metadata",
  "r:member.list",
  "r:account.list",
  "r:account.metadata",
  "x:account.metadata",
  "r:server.config",
  "r:policy.read",
  "w:message.create",
  "w:media.upload",
  "w:message.react",
  "w:message.edit.own",
  "w:message.edit.any",
  "w:message.retract.own",
  "w:message.retract.any",
  "w:room.create",
  "w:private.create",
  "w:account.rename.own",
  "w:account.color.own",
  "w:account.color.any",
  "x:account.rename.any",
  "w:account.password.own",
  "x:member.add",
  "x:member.remove",
  "x:room.join",
  "x:room.delete",
  "x:room.rename",
  "x:room.owner.transfer",
  "x:history.clean",
  "x:account.create",
  "x:account.password.reset",
  "x:account.disable",
  "x:account.enable",
  "x:account.delete",
  "x:group.admin.assign",
  "x:policy.change",
  "x:command.sudo",
  "x:bot.credentials",
  "r:bot.config",
  "w:bot.config",
];

/** Complete the token at the caret without replacing following arguments. */
export function cursorSuggestions(
  draft: string,
  caret: number,
  suggest: (prefix: string) => Hint[],
): { hints: Hint[]; suffix: string } {
  const prefix = draft.slice(0, caret);
  const tail = draft.slice(caret);
  // A bare @ starts a new mention before existing text, rather than replacing
  // the word to its right as completing a partially typed token would.
  const suffix =
    !prefix.startsWith("/") && prefix.endsWith("@")
      ? tail
      : tail.replace(/^\S*/u, "");
  return {
    suffix,
    hints: suggest(prefix).map((hint) => ({
      ...hint,
      value: hint.value
        ? /^\s/u.test(suffix)
          ? hint.value.trimEnd() + suffix
          : hint.value + suffix
        : null,
    })),
  };
}

export function nextHint(index: number, count: number, backwards = false) {
  return count ? (index + (backwards ? count - 1 : 1)) % count : 0;
}

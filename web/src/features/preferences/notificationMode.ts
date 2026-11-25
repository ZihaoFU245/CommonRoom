import type { Command, Hint } from "../../api/protocol.ts";

const operations = [
  "status",
  "install",
  "uninstall",
  "mute",
  "unmute",
  "filter",
];
const wholeFilter = ["all", "none", "default"];
const matchers = [
  "mentions_me",
  "mentions_all",
  "@all",
  "is_reply_to_me",
  "contains_media",
];
const matcherDescriptions = new Map([
  ["all", "Every eligible message"],
  ["none", "Suppress every alert in this scope"],
  ["default", "Restore the inherited filter"],
  ["@all", "Authorized room-wide mentions"],
  ["mentions_all", "Authorized room-wide mentions"],
]);
const actorPattern = /^(?:user_is|user_not|user_role):[^\s;]+$/u;

function completeRule(rule: string) {
  return (
    matchers.includes(rule) ||
    actorPattern.test(rule) ||
    /^(?:(?:user_is|user_not|user_role):[^\s;]+ +)?contains_keyword:\S[^;]*$/u.test(
      rule,
    )
  );
}

export function notificationSuggestions(
  draft: string,
  users: string[],
  rooms: string[],
  privateTargets: string[],
): Hint[] {
  const command = /^\/notifications\s+(.*)$/u.exec(draft)?.[1];
  if (command === undefined) return [];
  const make = (
    prefix: string,
    partial: string,
    values: string[],
    description: string,
    advance: boolean,
  ): Hint[] =>
    values
      .filter(
        (value) =>
          value.startsWith(partial) &&
          !(value === partial && value.endsWith(":")),
      )
      .map((value) => ({
        label: value,
        description: matcherDescriptions.get(value) ?? description,
        value: prefix + value + (value.endsWith(":") ? "" : " "),
        advance,
      }));
  if (!/\s/u.test(command))
    return make(
      draft.slice(0, draft.length - command.length),
      command,
      operations,
      "Notification operation",
      true,
    );

  const filter = /^filter\s+(.*)$/u.exec(command)?.[1];
  if (filter === undefined) return [];
  const prefix = draft.slice(0, draft.length - filter.length);
  const separator = filter.lastIndexOf(";");
  const earlier = filter.slice(0, separator + 1);
  const tail = filter.slice(separator + 1);
  const rule = tail.trimStart();
  const base = prefix + earlier + tail.slice(0, tail.length - rule.length);
  if (
    earlier &&
    !earlier
      .slice(0, -1)
      .split(";")
      .every((part) => completeRule(part.trim()))
  )
    return [];

  const scope = /^(.*\S)(\s+)(\S*)$/u.exec(rule);
  if (
    scope?.[1] &&
    (completeRule(scope[1]) || (!earlier && wholeFilter.includes(scope[1])))
  ) {
    const stem = base + scope[1] + scope[2];
    const partial = scope[3] ?? "";
    return [
      ...(actorPattern.test(scope[1])
        ? make(
            stem,
            partial,
            ["contains_keyword:"],
            "Add a keyword condition",
            true,
          )
        : []),
      ...make(
        stem,
        partial,
        [
          "here",
          "global",
          ...rooms.map((room) => `#${room}`),
          ...privateTargets,
        ],
        "Notification scope",
        false,
      ),
    ];
  }

  const actor = /^(user_is|user_not|user_role):([^\s]*)$/u.exec(rule);
  if (actor?.[1])
    return make(
      base + actor[1] + ":",
      actor[2] ?? "",
      actor[1] === "user_role" ? ["user", "admin", "su"] : users,
      "Notification actor",
      true,
    );

  return make(
    base,
    rule,
    [
      ...(earlier ? [] : wholeFilter),
      ...matchers,
      "user_is:",
      "user_not:",
      "user_role:",
      "contains_keyword:",
    ],
    "Notification matcher",
    true,
  );
}

export const notificationCommand: Command = {
  name: "/notifications",
  usage: "/notifications [status|install|uninstall|mute|unmute|filter]",
  description: "Manage subscriptions, mute alerts, or set a server-side filter",
  section: "General",
  admin: false,
  console: false,
};

export const notificationManual = [
  "# /notifications",
  "",
  "- `status` shows whether alerts are muted. Delete individual default matchers or saved scope filters, or reset all filters to defaults.",
  "- `install` and `uninstall` open the background subscription controls for this browser.",
  "- `mute` and `unmute` affect open-tab and background alerts on your account.",
  "- `filter <rules> <scope>` replaces the rule for a scope. Use `filter default <scope>` to clear it.",
  "",
  "Scopes are `here`, `global`, `#room`, or `@direct:username`. A conversation rule overrides the global rule.",
  "",
  "Separate matchers with semicolons. Put one scope at the end, after a space. Any matching rule alerts. Use `all` or `none` alone.",
  "Example: `/notifications filter mentions_me; @all #ops`.",
  "- Actor: `user_is:alice`, `user_not:alice`, `user_role:admin`",
  "- Target: `mentions_me`, `mentions_all` (or `@all`), `is_reply_to_me`",
  "- Content: `contains_keyword:incident`, `contains_media`",
  "- Actor and keyword: `user_is:alice contains_keyword:incident`",
  "",
  "By default, mentions, room-wide mentions, replies to your messages, and all incoming DMs alert.",
].join("\n");

import type { Message, Snapshot, Hint } from "../../api/protocol.ts";
function nameCharacter(value: string): boolean {
  return (
    /^[A-Za-z0-9_-]$/u.test(value) ||
    ((value.codePointAt(0) ?? 0) > 127 &&
      !/[\p{White_Space}\p{Cc}]/u.test(value))
  );
}
export function mentionSuggestions(draft: string, users: string[]): Hint[] {
  if (draft.startsWith("/")) return [];
  const index = draft.lastIndexOf("@");
  if (index < 0) return [];
  const preceding = Array.from(draft.slice(0, index)).at(-1);
  if (preceding && (nameCharacter(preceding) || preceding === "@")) return [];
  const prefix = draft.slice(index + 1);
  if (!Array.from(prefix).every(nameCharacter)) return [];
  return users
    .filter((name) => name.startsWith(prefix))
    .map((name) => ({
      label: `@${name}`,
      description: "Mention",
      value: draft.slice(0, draft.lastIndexOf("@")) + `@${name} `,
    }));
}
export function mentionEvents(previous: Snapshot, next: Snapshot) {
  const seen = new Set(
    [...previous.rooms.flatMap((r) => r.messages), ...previous.direct].map(
      (m) => m.id,
    ),
  );
  const previousRooms = new Map(previous.rooms.map((r) => [r.name, r]));
  const events: { message: Message; view: string | null }[] = [];
  function add(messages: Message[], older: Message[], view: string | null) {
    const newest = Math.max(0, ...older.map((m) => m.time));
    for (const message of messages) {
      if (
        !seen.has(message.id) &&
        message.time >= newest &&
        message.from !== next.username &&
        message.mentions?.includes(next.username)
      )
        events.push({ message, view });
    }
  }
  for (const room of next.rooms) {
    const previousRoom = previousRooms.get(room.name);
    if (previousRoom) add(room.messages, previousRoom.messages, room.name);
  }
  add(next.direct, previous.direct, null);
  return events.map((e) => ({
    ...e,
    view: e.view || `@direct:${e.message.from}`,
  }));
}
export function mentionedText(text: string, mentions: string[] = []) {
  const names = new Set(mentions);
  return text
    .split(
      /(@(?:(?![\p{White_Space}\p{Cc}])[A-Za-z0-9_\-\u0080-\u{10ffff}])+)/u,
    )
    .map((part, index, parts) => ({
      text: part,
      mention:
        part.startsWith("@") &&
        names.has(part.slice(1)) &&
        (!index ||
          !Array.from(parts[index - 1] || "")
            .slice(-1)
            .some((value) => nameCharacter(value) || value === "@")),
    }));
}

/** One run of message text: a mention, a clickable link, or plain text. */
export interface Token {
  text: string;
  mention: boolean;
  /** Address to open, or null for plain text. */
  href: string | null;
  /** Text to show; equals `text` for plain runs. */
  label: string;
}

// Trailing punctuation is trimmed by `linkHref`. Parentheses are excluded so a
// markdown link `[title](url)` does not swallow its own closing bracket; a URL
// that genuinely contains parentheses is therefore truncated at the first one.
const URL_PATTERN = /https?:\/\/[^\s<>"'`()]+/g;
// `[title](url)` as written by the server when it appends answer sources.
const MARKDOWN_LINK = /\[([^\]\n]{1,120})\]\((https?:\/\/[^\s)]+)\)/g;
/** Characters of a link label kept before it is elided. */
const LABEL = 48;

/**
 * The `href` for a link, or null when a client should not follow it. Only
 * http(s) URLs are links; everything else stays visible text.
 */
export function linkHref(value: string): string | null {
  // Trailing sentence punctuation is not part of the address.
  const trimmed = value.replace(/[.,;:!?)\]}'"»”]+$/, "");
  try {
    const url = new URL(trimmed);
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.href
      : null;
  } catch {
    return null;
  }
}

/** Shorten a link label so a long title does not dominate the message. */
function shortLabel(value: string): string {
  const label = value.split(/\s+/).filter(Boolean).join(" ");
  if (label.length <= LABEL) return label;
  return `${label.slice(0, LABEL).trimEnd()}…`;
}

/** A bare address shown as its own host and path rather than in full. */
function addressLabel(value: string): string {
  const trimmed = value.replace(/[.,;:!?)\]}'"»”]+$/, "");
  try {
    const url = new URL(trimmed);
    const path = url.pathname === "/" ? "" : url.pathname;
    return shortLabel(`${url.host}${path}${url.search ? "…" : ""}`);
  } catch {
    return shortLabel(value);
  }
}

function plainTokens(text: string): Token[] {
  const tokens: Token[] = [];
  let cursor = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    const start = match.index ?? 0;
    const href = linkHref(match[0]);
    if (!href) continue;
    if (start > cursor) {
      const plain = text.slice(cursor, start);
      tokens.push({ text: plain, mention: false, href: null, label: plain });
    }
    tokens.push({
      text: match[0],
      mention: false,
      href,
      label: addressLabel(match[0]),
    });
    cursor = start + match[0].length;
  }
  if (cursor < text.length) {
    const plain = text.slice(cursor);
    tokens.push({ text: plain, mention: false, href: null, label: plain });
  }
  return tokens;
}

/**
 * Message text as mentions, links, and plain runs. Links come from agent
 * answers citing search results, so they must be validated before rendering,
 * and a labelled link shows its title instead of its address.
 */
export function messageTokens(text: string, mentions: string[] = []): Token[] {
  const tokens: Token[] = [];
  for (const part of mentionedText(text, mentions)) {
    if (part.mention) {
      tokens.push({
        text: part.text,
        mention: true,
        href: null,
        label: part.text,
      });
      continue;
    }
    let cursor = 0;
    for (const match of part.text.matchAll(MARKDOWN_LINK)) {
      const start = match.index ?? 0;
      const title = match[1] || "";
      const href = linkHref(match[2] || "");
      if (!href) continue;
      if (start > cursor)
        tokens.push(...plainTokens(part.text.slice(cursor, start)));
      tokens.push({
        text: match[0],
        mention: false,
        href,
        label: shortLabel(title),
      });
      cursor = start + match[0].length;
    }
    if (cursor < part.text.length)
      tokens.push(...plainTokens(part.text.slice(cursor)));
  }
  return tokens.filter((token) => token.text.length > 0);
}

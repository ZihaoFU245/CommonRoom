import type { Message } from "../../api/protocol.ts";
export interface MessageDate {
  key: string;
  label: string;
  time: string;
  full: string;
  iso: string;
}
// No timeZone override: all visible dates follow the browser's timezone.
const clock = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});
const calendar = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
});
const fullDate = new Intl.DateTimeFormat(undefined, {
  dateStyle: "full",
  timeStyle: "long",
});
const dates = new Map<number, MessageDate>();
export function messageDate(seconds: number): MessageDate {
  const cached = dates.get(seconds);
  if (cached) return cached;
  const value = formatMessageDate(seconds);
  dates.set(seconds, value);
  if (dates.size > 2000) {
    const first = dates.keys().next().value;
    if (first !== undefined) dates.delete(first);
  }
  return value;
}
function formatMessageDate(seconds: number): MessageDate {
  const date = new Date(seconds * 1000);
  if (!Number.isFinite(date.getTime()))
    return {
      key: "invalid",
      label: "Unknown date",
      time: "",
      full: "Unknown date",
      iso: "",
    };
  return {
    key: `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`,
    label: calendar.format(date),
    time: clock.format(date),
    full: fullDate.format(date),
    iso: date.toISOString(),
  };
}
export function groupedMessage(
  message: Pick<Message, "from" | "time">,
  previous?: Pick<Message, "from" | "time"> | null,
) {
  return (
    !!previous &&
    message.from === previous.from &&
    message.time >= previous.time &&
    message.time - previous.time < 300 &&
    messageDate(message.time).key === messageDate(previous.time).key
  );
}

/** Visual bar runs follow the displayed minute, within one conversation. */
export function sameMinuteMessage(
  message: Pick<Message, "from" | "time"> &
    Partial<Pick<Message, "private_id" | "to">>,
  previous?:
    | (Pick<Message, "from" | "time"> &
        Partial<Pick<Message, "private_id" | "to">>)
    | null,
): boolean {
  return (
    !!previous &&
    groupedMessage(message, previous) &&
    Math.floor(message.time / 60) === Math.floor(previous.time / 60) &&
    message.private_id === previous.private_id &&
    message.to === previous.to
  );
}

import { useNameColor } from "./NameColorContext.ts";

/** Highlights `@name` in the mentioned account's color. */
export function Mention({
  text,
  roomWide = false,
}: {
  text: string;
  roomWide?: boolean;
}) {
  const color = useNameColor(text.slice(1));
  return (
    <mark class="mention" data-name-color={roomWide ? undefined : color}>
      {text}
    </mark>
  );
}

import { useNameColor } from "./NameColorContext.ts";

/** Highlights `@name` in the mentioned account's color. */
export function Mention({ text }: { text: string }) {
  return (
    <mark class="mention" data-name-color={useNameColor(text.slice(1))}>
      {text}
    </mark>
  );
}

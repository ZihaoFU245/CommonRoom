import { useMemo } from "preact/hooks";
import { mathMarkdownNodes } from "../features/conversation/mathMarkdown.ts";
import { MarkdownContent } from "./MarkdownContent.tsx";
export function MathMarkdown({
  text,
  mentions,
}: {
  text: string;
  mentions: string[];
}) {
  const nodes = useMemo(() => mathMarkdownNodes(text), [text]);
  return <MarkdownContent nodes={nodes} mentions={mentions} />;
}

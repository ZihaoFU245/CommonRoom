import type { AttachmentContext } from "./Attachment.tsx";
import { useMemo } from "preact/hooks";
import { mathMarkdownNodes } from "../features/conversation/mathMarkdown.ts";
import { MarkdownContent } from "./MarkdownContent.tsx";
import type { ComponentChildren } from "preact";
import {
  restoreHtmlFragments,
  type HtmlFragment,
} from "../features/conversation/htmlFragments.ts";
export function MathMarkdown({
  text,
  mentions,
  fragments = [],
  renderHtml,
  attachmentContext,
}: {
  text: string;
  mentions: string[];
  fragments?: HtmlFragment[];
  renderHtml?: (source: string, index: number) => ComponentChildren;
  attachmentContext?: AttachmentContext;
}) {
  const nodes = useMemo(
    () => restoreHtmlFragments(mathMarkdownNodes(text), fragments),
    [text, fragments],
  );
  return (
    <MarkdownContent
      nodes={nodes}
      mentions={mentions}
      {...(renderHtml ? { renderHtml } : {})}
      {...(attachmentContext ? { attachmentContext } : {})}
    />
  );
}

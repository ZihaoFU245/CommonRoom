import { lazy, memo, Suspense } from "preact/compat";
import { useMemo } from "preact/hooks";
import { markdownNodes } from "../features/conversation/markdown.ts";
import { HtmlSource } from "./HtmlSource.tsx";
import { HtmlCover, type HtmlCoverProps } from "./HtmlCover.tsx";
import type { ComponentChildren } from "preact";
import {
  htmlFragments,
  restoreHtmlFragments,
} from "../features/conversation/htmlFragments.ts";
import { MarkdownContent } from "./MarkdownContent.tsx";
const HtmlMarkdown = lazy(() =>
  import("./HtmlMarkdownGate.tsx")
    .then((module) => ({
      default: module.HtmlMarkdownGate,
    }))
    .catch(() => ({
      default: (props: HtmlCoverProps & { fallback: ComponentChildren }) => {
        const error = (
          <p role="status">Unable to load HTML preview. Reload to retry.</p>
        );
        return <HtmlCover {...props}>{error}</HtmlCover>;
      },
    })),
);
const MathMarkdown = lazy(() =>
  import("./MathMarkdown.tsx")
    .then((module) => ({
      default: module.MathMarkdown,
    }))
    .catch(() => ({ default: ({ text }: { text: string }) => <p>{text}</p> })),
);
export interface MarkdownProps {
  text: string;
  mentions: string[];
  messageKey: string;
  allowHtml: boolean;
  latex: boolean;
}
export const Markdown = memo(function Markdown(props: MarkdownProps) {
  const parts = useMemo(() => htmlFragments(props.text), [props.text]);
  const nodes = useMemo(
    () => restoreHtmlFragments(markdownNodes(parts.markdown), parts.fragments),
    [parts],
  );
  const renderHtml = (source: string, index: number) => {
    if (!props.allowHtml) return <HtmlSource key={index} text={source} />;
    const cover: HtmlCoverProps = {
      text: source,
      messageKey: props.messageKey,
      choiceKey: JSON.stringify([props.messageKey, index]),
      consentText: props.text,
    };
    const fallback = <HtmlSource text={source} />;
    return (
      <Suspense key={index} fallback={<HtmlCover {...cover} />}>
        <HtmlMarkdown {...cover} fallback={fallback} />
      </Suspense>
    );
  };
  const attachmentContext = {
    messageKey: props.messageKey,
    consentText: props.text,
    allowHtml: props.allowHtml,
  };
  const plain = (
    <MarkdownContent
      nodes={nodes}
      mentions={props.mentions}
      renderHtml={renderHtml}
      attachmentContext={attachmentContext}
    />
  );
  return props.latex ? (
    <Suspense fallback={plain}>
      <MathMarkdown
        text={parts.markdown}
        mentions={props.mentions}
        fragments={parts.fragments}
        renderHtml={renderHtml}
        attachmentContext={attachmentContext}
      />
    </Suspense>
  ) : (
    plain
  );
});

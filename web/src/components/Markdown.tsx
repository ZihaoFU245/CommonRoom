import { lazy, memo, Suspense } from "preact/compat";
import { useMemo } from "preact/hooks";
import {
  markdownNodes,
  hasRawHtml,
} from "../features/conversation/markdown.ts";
import { HtmlCover } from "./HtmlCover.tsx";
import { MarkdownContent } from "./MarkdownContent.tsx";
const HtmlMarkdown = lazy(() =>
  import("./HtmlMarkdownGate.tsx")
    .then((module) => ({
      default: module.HtmlMarkdownGate,
    }))
    .catch(() => ({
      default: (
        props: MarkdownProps & { fallback: import("preact").ComponentChildren },
      ) => {
        const error = (
          <p role="status">Unable to load HTML preview. Reload to retry.</p>
        );
        return props.allowHtmlCode ? (
          <HtmlCover text={props.text} messageKey={props.messageKey}>
            {error}
          </HtmlCover>
        ) : (
          <>
            {props.fallback}
            {error}
          </>
        );
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
  allowHtmlCode: boolean;
  latex: boolean;
}
export const Markdown = memo(function Markdown(props: MarkdownProps) {
  const nodes = useMemo(() => markdownNodes(props.text), [props.text]);
  const plain = <MarkdownContent nodes={nodes} mentions={props.mentions} />;
  const content = props.latex ? (
    <Suspense fallback={plain}>
      <MathMarkdown text={props.text} mentions={props.mentions} />
    </Suspense>
  ) : (
    plain
  );
  const html = useMemo(
    () => (props.allowHtml || props.allowHtmlCode) && hasRawHtml(props.text),
    [props.allowHtml, props.allowHtmlCode, props.text],
  );
  if (html)
    return (
      <Suspense
        fallback={
          props.allowHtmlCode ? (
            <HtmlCover text={props.text} messageKey={props.messageKey} />
          ) : (
            <p role="status">Loading HTML preview…</p>
          )
        }
      >
        <HtmlMarkdown {...props} fallback={content} />
      </Suspense>
    );
  return content;
});
